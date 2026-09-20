//! Windows backend for [`super::Transport`].
//!
//! True process cloning via `RtlCloneUserProcess`; `ipc-channel` for both
//! planes, as on macOS. What differs is *who* the clone talks to:
//!
//! - **Control plane** (Runtime ↔ Main Zygote): one `IpcOneShotServer` set
//!   up in the Runtime; Main Zygote connects with `IpcSender::connect(name)`,
//!   hands over its `IpcSender<ZygoteCommand>` + `IpcReceiver<ZygoteReply>`,
//!   and the one-shot is dropped.
//!
//! - **Data plane** (Runtime ↔ Clone): the Runtime creates both channel pairs
//!   itself. Per clone: the Runtime listens on a one-shot server and passes
//!   its name in `SpawnClone`; Main Zygote only clones and answers with the
//!   pid; the clone opens its own short-lived setup server and tells the
//!   Runtime its name; the Runtime sends the clone-side ends there.
//!
//! # Why the Runtime is the rendezvous point
//!
//! `RtlCloneUserProcess` copies the memory of Main Zygote, and `ipc-channel`
//! keeps process-wide state in it: its cached pid (`CURRENT_PROCESS_ID`,
//! a `LazyLock`) is already resolved to the pid of Main Zygote by its own
//! bootstrap `send`. A clone that sends channel ends to a pipe owned by Main
//! Zygote takes that pipe for its own (`server pid == cached pid`) and
//! duplicates the handles into itself instead of into Main Zygote. A pipe
//! owned by the Runtime does not match the stale pid, so the same `send`
//! works. Main Zygote never sends channel ends after its bootstrap.
//!
//! # Why the Runtime creates the long-lived channels
//!
//! Pipe names are UUIDs, and the RNG behind them (`ProcessPrng`) is copied
//! together with the rest of the memory: clones of one Main Zygote draw the
//! same UUIDs. Names chosen by a clone would collide with those of its
//! siblings, and `ipc-channel` keeps received handles open, so a dead clone
//! would go on occupying its names. The names of the long-lived channels come
//! from the Runtime instead (an ordinary process), and the clone only creates
//! a setup server that lives for the duration of the bootstrap
//! (`low::decorrelateRandom` moves it to its own place in the stream).
// =================================================================================================
use super::Transport as TransportTrait;
use super::{
  CloneSide as CloneSideTrait, FFIRequest, FFIResponse,
  RuntimeSide as RuntimeSideTrait, ZygoteHandleBase
};
use crate::ffi::errors::FFIError;
use crate::platform::low;
use crate::worker::executeFFI;
use crate::worker::{takeLastErrno, takeLastOsError};
use crate::zygote::ZygoteFlag;
use fxhash::FxHashMap;
use ipc_channel::ipc::{self, IpcOneShotServer, IpcReceiver, IpcSender};
use libloading::Library;
use serde::{Deserialize, Serialize};
use std::env;
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};
// =================================================================================================

/// Hidden startup flag of a legacy Command-based clone (kept for
/// compatibility — current path is `RtlCloneUserProcess`).
pub const CloneFlag: &str = "__zygoteClone";

/// Backend tag used in diagnostics.
const BackendName: &str = "windows-rtlcloneuserprocess";

/// How long a clone may take to greet the Runtime.
const CloneBootstrapTimeout: Duration = Duration::from_secs(20);

/// Exit codes of a clone that failed during its bootstrap. Nobody reads the
/// stderr of a clone reliably; the Runtime puts the code into its error.
const CloneExitSetupServer: u32 = 11;
const CloneExitConnect: u32 = 12;
const CloneExitHello: u32 = 13;
const CloneExitSetup: u32 = 14;

// =================================================================================================

/// Commands Runtime → Main Zygote.
#[derive(Serialize, Deserialize)]
pub enum ZygoteCommand
{
  /// Ask Main Zygote to `RtlCloneUserProcess` a clone.
  ///
  /// `bootstrapName` is the name of the [`IpcOneShotServer`] that the
  /// *Runtime* listens on: the clone connects to it directly and greets the
  /// Runtime, Main Zygote never touches the data channels.
  SpawnClone { bootstrapName: String }
}

/// Replies Main Zygote → Runtime.
#[derive(Serialize, Deserialize)]
pub enum ZygoteReply
{
  /// The clone process exists; only its pid. No channel ends pass through
  /// Main Zygote — the clone and the Runtime settle them between themselves.
  Cloned { pid: u32 },

  /// `cloneProcess()` failed inside Main Zygote.
  SpawnFailed
}

/// First message from Zygote after connecting to Runtime's [`IpcOneShotServer`].
#[derive(Serialize, Deserialize)]
struct BootstrapToRuntime
{
  /// Runtime → Main Zygote commands.
  commandTx: IpcSender<ZygoteCommand>,

  /// Main Zygote → Runtime replies.
  replyRx: IpcReceiver<ZygoteReply>
}

/// First message from a freshly cloned process → Runtime: "I am up, my
/// setup server is called `setupName`".
#[derive(Serialize, Deserialize)]
struct CloneHello
{
  /// Name of the clone's one-shot setup server.
  setupName: String
}

/// Runtime → clone, over the setup server of the clone: the clone-side ends
/// of the two data channels the Runtime has created.
#[derive(Serialize, Deserialize)]
struct CloneSetup
{
  /// Runtime → Clone requests.
  requestRx: IpcReceiver<FFIRequest>,

  /// Clone → Runtime responses.
  responseTx: IpcSender<FFIResponse>
}

// =================================================================================================

/// Windows Transport: `RtlCloneUserProcess` + ipc-channel.
pub struct Transport;

/// Runtime-side handle to the Main Zygote.
pub struct ZygoteHandle
{
  /// Common handle (process handle + Drop).
  pub base: ZygoteHandleBase,

  /// Runtime → Main Zygote commands.
  pub commandTx: IpcSender<ZygoteCommand>,

  /// Main Zygote → Runtime replies.
  pub replyRx: IpcReceiver<ZygoteReply>
}

/// Runtime-side data endpoint.
pub struct RuntimeSide
{
  /// Pid of the clone, to say what became of it when the channel breaks.
  pub pid: u32,

  /// Runtime → Clone requests.
  pub requestTx: IpcSender<FFIRequest>,

  /// Clone → Runtime responses.
  pub responseRx: IpcReceiver<FFIResponse>
}

/// Clone-side data endpoint.
pub struct CloneSide
{
  /// Runtime → Clone requests.
  pub requestRx: IpcReceiver<FFIRequest>,

  /// Clone → Runtime responses.
  pub responseTx: IpcSender<FFIResponse>
}

/// Clone spawn result: pid and Runtime-side data channel ends.
#[derive(Serialize, Deserialize)]
pub struct Bootstrap
{
  /// PID of the freshly created clone.
  pub pid: u32,

  /// Runtime → Clone requests.
  pub requestTx: IpcSender<FFIRequest>,

  /// Clone → Runtime responses.
  pub responseRx: IpcReceiver<FFIResponse>
}

// =================================================================================================

impl TransportTrait for Transport
{
  type RuntimeSide = RuntimeSide;
  type CloneSide = CloneSide;
  type Bootstrap = Bootstrap;
  type ZygoteHandle = ZygoteHandle;

  /// Short backend tag for diagnostics. Dispatched through the trait, so
  /// Clippy sees it as "never used" — silenced here.
  #[allow(dead_code)]
  fn name() -> &'static str
  {
    BackendName
  }

  /// Spawns the Main Zygote and bootstraps the control channel.
  fn spawnZygote() -> io::Result<Self::ZygoteHandle>
  {
    let (server, serverName): (
      IpcOneShotServer<BootstrapToRuntime>,
      String
    ) = IpcOneShotServer::new().map_err(io::Error::other)?;

    let currentExe: PathBuf = env::current_exe()?;
    let process: Child = Command::new(currentExe)
      .arg(ZygoteFlag)
      .arg(&serverName)
      .stdin(Stdio::null())
      .stdout(Stdio::inherit())
      .stderr(Stdio::inherit())
      .spawn()?;

    let (_rx, bootstrap): (
      IpcReceiver<BootstrapToRuntime>,
      BootstrapToRuntime
    ) = server.accept().map_err(|e| {
      io::Error::other(format!("zygote bootstrap accept: {e}"))
    })?;

    Ok(ZygoteHandle {
      base: ZygoteHandleBase { process },
      commandTx: bootstrap.commandTx,
      replyRx: bootstrap.replyRx
    })
  }

  /// Runtime asks Main Zygote to clone a process and hands the clone its
  /// channel ends (see the module docs).
  fn sendSpawnClone(handle: &Self::ZygoteHandle) -> io::Result<Self::Bootstrap>
  {
    // The long-lived channels: created here, named by the Runtime's RNG.
    let (requestTx, requestRx): (
      IpcSender<FFIRequest>,
      IpcReceiver<FFIRequest>
    ) = ipc::channel::<FFIRequest>()?;
    let (responseTx, responseRx): (
      IpcSender<FFIResponse>,
      IpcReceiver<FFIResponse>
    ) = ipc::channel::<FFIResponse>()?;

    let (server, serverName): (
      IpcOneShotServer<CloneHello>,
      String
    ) = IpcOneShotServer::new().map_err(io::Error::other)?;

    handle
      .commandTx
      .send(ZygoteCommand::SpawnClone { bootstrapName: serverName.clone() })
      .map_err(|e| {
        io::Error::new(
          io::ErrorKind::BrokenPipe,
          format!("SpawnClone send failed: {e}")
        )
      })?;

    let reply: ZygoteReply = handle.replyRx.recv().map_err(|e| {
      io::Error::new(
        io::ErrorKind::BrokenPipe,
        format!("SpawnClone reply failed: {e}")
      )
    })?;

    let pid: u32 = match reply
    {
      ZygoteReply::Cloned { pid } => pid,
      ZygoteReply::SpawnFailed => return Err(io::Error::other(
        "Main zygote failed to create a clone (RtlCloneUserProcess failed)"
      ))
    };
    if pid == 0
    {
      return Err(io::Error::other(
        "Main zygote failed to create a clone (pid=0)"
      ));
    }

    // The clone is up and has opened its setup server.
    let hello: CloneHello = acceptHello(server, &serverName, pid)?;

    // Hand it its ends. The clone owns the server of this pipe, so
    // ipc-channel duplicates the handles straight into the clone.
    let setupTx: IpcSender<CloneSetup> = IpcSender::connect(hello.setupName)
      .map_err(|e| {
        low::killProcess(pid);
        io::Error::other(format!(
          "connecting to the setup server of clone {pid} failed: {e} ({})",
          cloneStatus(pid)
        ))
      })?;
    setupTx
      .send(CloneSetup { requestRx, responseTx })
      .map_err(|e| {
        low::killProcess(pid);
        io::Error::other(format!(
          "sending the channels to clone {pid} failed: {e} ({})",
          cloneStatus(pid)
        ))
      })?;
    drop(setupTx);

    Ok(Bootstrap { pid, requestTx, responseRx })
  }

  /// PID stored in the bootstrap message.
  fn bootstrapPid(bootstrap: &Self::Bootstrap) -> u32
  {
    bootstrap.pid
  }

  /// Enters the Main Zygote command loop.
  ///
  /// `flag`: the `IpcOneShotServer` name passed as `argv[2]`.
  fn zygoteControlLoop(flag: Option<String>) -> !
  {
    let serverName: String =
      flag.expect("windows::zygoteControlLoop: missing IpcOneShotServer name");
    zygoteLoop(serverName)
  }

  /// In a freshly cloned process: prepares the data endpoint. Dispatched
  /// through the trait, so Clippy sees it as "never used" — silenced here.
  #[allow(dead_code)]
  fn cloneEnter(flag: Option<String>) -> io::Result<(Self::CloneSide, Self::Bootstrap)>
  {
    let serverName: String =
      flag.expect("windows::cloneEnter: missing IpcOneShotServer name");
    cloneBootstrapLoop(serverName)
  }

  /// Turns a bootstrap message into a Runtime-side data endpoint.
  fn runtimeConnect(bootstrap: Self::Bootstrap) -> io::Result<Self::RuntimeSide>
  {
    Ok(RuntimeSide {
      pid: bootstrap.pid,
      requestTx: bootstrap.requestTx,
      responseRx: bootstrap.responseRx
    })
  }
}

// =================================================================================================

impl RuntimeSideTrait for RuntimeSide
{
  /// Sends an FFI request to the clone.
  fn send(&self, request: &FFIRequest) -> Result<(), String>
  {
    self
      .requestTx
      .send(request.clone())
      .map_err(|e| {
        format!(
          "Zygote clone IPC failed while sending request: {e} ({})",
          cloneStatus(self.pid)
        )
      })
  }

  /// Receives an FFI response from the clone.
  fn recv(&self) -> Result<FFIResponse, String>
  {
    self
      .responseRx
      .recv()
      .map_err(|e| {
        format!(
          "Zygote clone IPC failed while reading response: {e} ({})",
          cloneStatus(self.pid)
        )
      })
  }
}

impl CloneSideTrait for CloneSide
{
  /// Dispatched through the trait, so Clippy sees it as "never used" —
  /// silenced here.
  #[allow(dead_code)]
  fn run(self, cache: &mut FxHashMap<String, Library>) -> !
  {
    let Self { requestRx, responseTx } = self;
    let mut libraryCache: FxHashMap<String, Library> = std::mem::take(cache);

    loop
    {
      let request: FFIRequest = match requestRx.recv()
      {
        Ok(r) => r,
        Err(_) => std::process::exit(0)
      };

      // catch_unwind: a panic inside the clone must not abort the process
      // (the Runtime would only see a dead channel). Convert to
      // FFIResponse::Err. Note: true AVs / SEH still kill the process —
      // that is intentional isolation.
      let response: FFIResponse =
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
          handleRequest(request, &mut libraryCache)
        })) {
          Ok(r) => r,
          Err(_) => FFIResponse::Err(FFIError::Other(
            "clone panicked while handling request".into()
          ))
        };

      if responseTx.send(response).is_err() {
        std::process::exit(0);
      }
    }
  }
}

// =================================================================================================

/// Human-readable fate of a clone, for error messages.
fn cloneStatus(pid: u32) -> String
{
  match low::processExitCode(pid)
  {
    None => format!("clone {pid} is running"),
    Some(code) => format!(
      "clone {pid} exited with code {code:#x}: {}",
      cloneExitReason(code)
    )
  }
}

/// What an exit code of a clone most likely means.
const fn cloneExitReason(code: u32) -> &'static str
{
  match code
  {
    CloneExitSetupServer => "could not create its setup server",
    CloneExitConnect => "could not connect to the Runtime",
    CloneExitHello => "could not send its greeting to the Runtime",
    CloneExitSetup => "did not receive its channels",
    1 => "killed by the Runtime",
    0xC000_0005 => "access violation",
    0xC000_0409 => "fast fail (abort, or a panic that cannot unwind)",
    u32::MAX => "gone before it could be queried",
    _ => "unknown"
  }
}

/// Waits for the greeting of the clone `pid`.
///
/// `IpcOneShotServer::accept` has no timeout, and a clone that died (or hung)
/// before connecting would block the Runtime forever. A watchdog thread
/// watches the clone and, once it exits or [`CloneBootstrapTimeout`] passes,
/// unblocks `accept` by connecting to the server and dropping the connection.
/// The failure then carries the reason, including the exit code of the clone.
fn acceptHello(
  server: IpcOneShotServer<CloneHello>,
  serverName: &str,
  pid: u32
) -> io::Result<CloneHello>
{
  let finished: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
  let watchdogFinished: Arc<AtomicBool> = Arc::clone(&finished);
  let watchdogName: String = serverName.to_owned();

  let spawned: io::Result<thread::JoinHandle<Option<String>>> = thread::Builder::new()
    .name("chillffi-clone-watchdog".into())
    .spawn(move || {
      let deadline: Instant = Instant::now() + CloneBootstrapTimeout;
      while !watchdogFinished.load(Ordering::Acquire)
      {
        let reason: Option<String> = if low::processExitCode(pid).is_some()
        {
          Some(format!("{} before bootstrap", cloneStatus(pid)))
        }
        else if Instant::now() >= deadline
        {
          Some(format!("clone {pid} did not bootstrap within {CloneBootstrapTimeout:?}"))
        }
        else
        {
          None
        };
        if let Some(reason) = reason
        {
          low::killProcess(pid);
          // A connection that carries nothing makes `accept` fail.
          let _ = IpcSender::<CloneHello>::connect(watchdogName);
          return Some(reason);
        }
        thread::sleep(Duration::from_millis(10));
      }
      None
    });
  let watchdog: thread::JoinHandle<Option<String>> = match spawned
  {
    Ok(handle) => handle,
    Err(e) =>
    {
      low::killProcess(pid);
      return Err(io::Error::other(format!("clone watchdog spawn failed: {e}")));
    }
  };

  let accepted = server.accept();
  finished.store(true, Ordering::Release);
  let reason: Option<String> = watchdog.join().unwrap_or(None);

  match accepted
  {
    Ok((_rx, hello)) => Ok(hello),
    Err(e) =>
    {
      // Never leave a clone nobody owns.
      low::killProcess(pid);
      Err(io::Error::other(
        reason.unwrap_or_else(|| format!("clone {pid} bootstrap accept failed: {e}"))
      ))
    }
  }
}

// =================================================================================================

/// Handles an incoming request and performs an FFI operation using the library cache.
fn handleRequest(request: FFIRequest, cache: &mut FxHashMap<String, Library>) -> FFIResponse
{
  match executeFFI(request, cache)
  {
    Ok(v) => FFIResponse::Ok(v, takeLastErrno(), takeLastOsError()),
    Err(e) => FFIResponse::Err(e)
  }
}

// =================================================================================================

/// Main zygote loop.
fn zygoteLoop(serverName: String) -> !
{
  low::ignoreChildExits();

  // Resolve the ntdll CSR data block [CsrServerApiRoutine .. RtlpEnvironLookupTable)
  // and kernelbase!CtrlRoutine once, in the healthy zygote, BEFORE any clone is
  // spawned. Children inherit the cached addresses via CoW and use them in
  // `reconnectCsr()` to zero the stale block, call CsrClientConnectToServer
  // for BASESRV + USERSRV, and RtlRegisterThreadWithCsrss. No-op on Unix.
  low::resolveCsrPortHandle();

  let (commandTx, commandRx): (
    IpcSender<ZygoteCommand>,
    IpcReceiver<ZygoteCommand>
  ) = match ipc::channel::<ZygoteCommand>() {
    Ok(p) => p,
    Err(_) => std::process::exit(1)
  };
  let (replyTx, replyRx): (IpcSender<ZygoteReply>, IpcReceiver<ZygoteReply>) =
    match ipc::channel::<ZygoteReply>() {
      Ok(p) => p,
      Err(_) => std::process::exit(1)
    };

  let bootstrapTx: IpcSender<BootstrapToRuntime> =
    match IpcSender::connect(serverName) {
      Ok(tx) => tx,
      Err(_) => std::process::exit(1)
    };
  if bootstrapTx
    .send(BootstrapToRuntime { commandTx, replyRx })
    .is_err()
  {
    std::process::exit(1);
  }
  drop(bootstrapTx);

  loop
  {
    let cmd: ZygoteCommand = match commandRx.recv() {
      Ok(c) => c,
      Err(_) => std::process::exit(0)
    };

    match cmd
    {
      ZygoteCommand::SpawnClone { bootstrapName } =>
      {
        match low::cloneProcess() {
          Ok(result) => {
            let pid: low::ProcessId = result.pid;
            // Thread already running (no CREATE_SUSPENDED).
            low::closeCloneHandles(&result);
            let _ = replyTx.send(ZygoteReply::Cloned { pid });
          }
          Err(low::StatusProcessCloned) => {
            std::mem::forget(commandRx);
            std::mem::forget(replyTx);

            // We are the clone. The inherited ntdll CSR data block
            // (CsrPortHandle, CsrInitOnceDone, CsrPortHeap, CsrHeap, ...)
            // references the parent's CSR_PROCESS on the csrss.exe side.
            // Any Win32/basesrv call (reattachConsole, _stat64 in
            // handleRequest, etc.) AVs and we die with ERROR_BROKEN_PIPE
            // (109). reconnectCsr() zeroes the whole block, calls
            // CsrClientConnectToServer for BASESRV + USERSRV against
            // \Sessions\{sid}\Windows, and registers the current thread
            // with RtlRegisterThreadWithCsrss. Best-effort: if symbols
            // were never resolved, we proceed anyway — same failure mode
            // as before this fix.
            let csrOk: bool = low::reconnectCsr();

            // reattachConsole goes through Win32 → CSRSS. If CSR was
            // not reconnected (ARM64 without a resolved block), the
            // stale ALPC port makes FreeConsole/AttachConsole hang —
            // the clone never reaches cloneBootstrapLoop, and the
            // Runtime waits for the bootstrap until its watchdog fires.
            if csrOk {
              low::reattachConsole();
            }
            low::silenceCrashReporting();
            cloneBootstrapLoop(bootstrapName)
          }
          Err(_) => {
            let _ = replyTx.send(ZygoteReply::SpawnFailed);
          }
        }
      }
    }
  }
}

/// Exits a clone that failed during its bootstrap.
fn cloneExit(code: u32) -> !
{
  std::process::exit(code as i32)
}

/// Bootstrap loop in a freshly cloned process.
///
/// Nothing that was created before the clone is usable in it (its handle
/// table is empty), so it starts from scratch: opens a setup server, greets
/// the Runtime (`helloName`) with the name of it, and receives its data
/// channels there.
fn cloneBootstrapLoop(helloName: String) -> !
{
  // Before the first UUID is drawn: see the module docs.
  low::decorrelateRandom();

  let (setupServer, setupName): (
    IpcOneShotServer<CloneSetup>,
    String
  ) = match IpcOneShotServer::new() {
    Ok(v) => v,
    Err(e) => {
      eprintln!("[clone] setup server failed: {e}");
      cloneExit(CloneExitSetupServer)
    }
  };

  // The Runtime owns this server (see the module docs).
  let helloTx: IpcSender<CloneHello> = match IpcSender::connect(helloName) {
    Ok(tx) => tx,
    Err(e) => {
      eprintln!("[clone] connect to the Runtime failed: {e}");
      cloneExit(CloneExitConnect)
    }
  };
  if let Err(e) = helloTx.send(CloneHello { setupName })
  {
    eprintln!("[clone] greeting failed: {e}");
    cloneExit(CloneExitHello);
  }
  drop(helloTx);

  let (setupRx, setup): (IpcReceiver<CloneSetup>, CloneSetup) =
    match setupServer.accept() {
      Ok(v) => v,
      Err(e) => {
        eprintln!("[clone] receiving the channels failed: {e}");
        cloneExit(CloneExitSetup)
      }
    };
  // Free the name of the setup server: the next clone may draw the same one.
  drop(setupRx);

  let cache: &mut FxHashMap<String, Library> =
    Box::leak(Box::new(FxHashMap::default()));
  CloneSide {
    requestRx: setup.requestRx,
    responseTx: setup.responseTx
  }.run(cache)
}

/// Legacy Command-based clone entry (kept for compatibility).
pub fn runAsClone() -> !
{
  let serverName: String = env::args()
    .nth(2)
    .expect("zygote clone: missing IpcOneShotServer name (argv[2])");

  low::silenceCrashReporting();
  cloneBootstrapLoop(serverName)
}

// =================================================================================================
