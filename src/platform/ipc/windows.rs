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
//! - **Data plane** (Runtime ↔ Clone): per-clone `IpcOneShotServer` owned by
//!   the *Runtime*. Its name travels in `SpawnClone`, Main Zygote only clones
//!   and answers with the pid. The clone creates a fresh `ipc::channel()`
//!   pair, connects to the Runtime and sends the Runtime-facing ends itself.
//!
//! # Why the Runtime and not Main Zygote is the rendezvous point
//!
//! `RtlCloneUserProcess` copies the memory of Main Zygote, and `ipc-channel`
//! keeps process-wide state in it: its cached pid (`CURRENT_PROCESS_ID`,
//! a `LazyLock`) is already resolved to the pid of Main Zygote by its own
//! bootstrap `send`. A clone that sends channel ends to a pipe owned by Main
//! Zygote takes that pipe for its own (`server pid == cached pid`) and
//! duplicates the handles into itself instead of into Main Zygote. A pipe
//! owned by the Runtime does not match the stale pid, so the same `send`
//! works. Main Zygote never sends channel ends after its bootstrap.
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

/// How long a clone may take to reach its bootstrap `send`.
const CloneBootstrapTimeout: Duration = Duration::from_secs(20);

/// Exit codes of a clone that failed before it could report back. Nobody
/// reads the stderr of a clone reliably; the Runtime puts the code into its error.
const CloneExitRequestChannel: i32 = 11;
const CloneExitResponseChannel: i32 = 12;
const CloneExitConnect: i32 = 13;
const CloneExitSend: i32 = 14;

// =================================================================================================

/// Commands Runtime → Main Zygote.
#[derive(Serialize, Deserialize)]
pub enum ZygoteCommand
{
  /// Ask Main Zygote to `RtlCloneUserProcess` a clone.
  ///
  /// `bootstrapName` is the name of the [`IpcOneShotServer`] that the
  /// *Runtime* listens on: the clone connects to it directly and hands over
  /// its channel ends itself, Main Zygote never touches them.
  SpawnClone { bootstrapName: String }
}

/// Replies Main Zygote → Runtime.
#[derive(Serialize, Deserialize)]
pub enum ZygoteReply
{
  /// The clone process exists; only its pid. The channel ends do not pass
  /// through Main Zygote — the clone sends them straight to the Runtime.
  Cloned { pid: u32 },

  /// `cloneProcess()` failed inside Main Zygote.
  SpawnFailed
}

/// First message from Zygote after connecting to Runtime's [`IpcOneShotServer`].
#[derive(Serialize, Deserialize)]
struct BootstrapToRuntime
{
  /// todo desc
  commandTx: IpcSender<ZygoteCommand>,

  /// todo desc
  replyRx: IpcReceiver<ZygoteReply>
}

/// First message from a freshly cloned process → Runtime.
/// Carries the ends that Runtime will use; the clone keeps the opposite ends.
#[derive(Serialize, Deserialize)]
struct CloneBootstrap
{
  /// todo desc
  requestTx: IpcSender<FFIRequest>,

  /// todo desc
  responseRx: IpcReceiver<FFIResponse>
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

/// Bootstrap of a freshly cloned process, as the Runtime holds it.
#[derive(Serialize, Deserialize)]
pub struct Bootstrap
{
  /// todo desc
  pub pid: u32,

  /// todo desc
  pub requestTx: IpcSender<FFIRequest>,

  /// todo desc
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

  /// Runtime asks Main Zygote to clone a process and receives the clone's
  /// channel ends straight from the clone (see the module docs).
  fn sendSpawnClone(handle: &Self::ZygoteHandle) -> io::Result<Self::Bootstrap>
  {
    let (server, serverName): (
      IpcOneShotServer<CloneBootstrap>,
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

    let bootstrap: CloneBootstrap = acceptCloneBootstrap(server, &serverName, pid)?;
    Ok(Bootstrap {
      pid,
      requestTx: bootstrap.requestTx,
      responseRx: bootstrap.responseRx
    })
  }

  /// todo desc
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

  /// todo desc
  fn runtimeConnect(bootstrap: Self::Bootstrap) -> io::Result<Self::RuntimeSide>
  {
    Ok(RuntimeSide {
      requestTx: bootstrap.requestTx,
      responseRx: bootstrap.responseRx
    })
  }
}

// =================================================================================================

impl RuntimeSideTrait for RuntimeSide
{
  /// todo desc
  fn send(&self, request: &FFIRequest) -> Result<(), String>
  {
    self
      .requestTx
      .send(request.clone())
      .map_err(|e| format!("Zygote clone IPC failed while sending request: {e}"))
  }

  /// todo desc
  fn recv(&self) -> Result<FFIResponse, String>
  {
    self
      .responseRx
      .recv()
      .map_err(|e| format!("Zygote clone IPC failed while reading response: {e}"))
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

/// Waits for the bootstrap message of the clone `pid`.
///
/// `IpcOneShotServer::accept` has no timeout, and a clone that died (or hung)
/// before connecting would block the Runtime forever. A watchdog thread
/// watches the clone and, once it exits or [`CloneBootstrapTimeout`] passes,
/// unblocks `accept` by connecting to the server and dropping the connection.
/// The failure then carries the reason, including the exit code of the clone.
fn acceptCloneBootstrap(
  server: IpcOneShotServer<CloneBootstrap>,
  serverName: &str,
  pid: u32
) -> io::Result<CloneBootstrap>
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
        let reason: Option<String> = match low::processExitCode(pid)
        {
          Some(code) => Some(format!("clone {pid} exited with code {code:#x} before bootstrap")),
          None if Instant::now() >= deadline => Some(
            format!("clone {pid} did not bootstrap within {CloneBootstrapTimeout:?}")
          ),
          None => None
        };
        if let Some(reason) = reason
        {
          low::killProcess(pid);
          // A connection that carries nothing makes `accept` fail.
          let _ = IpcSender::<CloneBootstrap>::connect(watchdogName);
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
    Ok((_rx, bootstrap)) => Ok(bootstrap),
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

/// Bootstrap loop in a freshly cloned process.
///
/// The channels must be created here, after the clone: handles created
/// before it do not exist in the clone (its handle table is empty).
fn cloneBootstrapLoop(serverName: String) -> !
{
  let (requestTx, requestRx): (
    IpcSender<FFIRequest>,
    IpcReceiver<FFIRequest>
  ) = match ipc::channel::<FFIRequest>() {
    Ok(p) => p,
    Err(e) => {
      eprintln!("[clone] request channel failed: {e}");
      std::process::exit(CloneExitRequestChannel)
    }
  };
  let (responseTx, responseRx): (
    IpcSender<FFIResponse>,
    IpcReceiver<FFIResponse>
  ) = match ipc::channel::<FFIResponse>() {
    Ok(p) => p,
    Err(e) => {
      eprintln!("[clone] response channel failed: {e}");
      std::process::exit(CloneExitResponseChannel)
    }
  };

  // The server belongs to the Runtime (see the module docs).
  let bootstrapTx: IpcSender<CloneBootstrap> =
    match IpcSender::connect(serverName) {
      Ok(tx) => tx,
      Err(e) => {
        eprintln!("[clone] bootstrap connect failed: {e}");
        std::process::exit(CloneExitConnect)
      }
    };
  if let Err(e) = bootstrapTx.send(CloneBootstrap { requestTx, responseRx })
  {
    eprintln!("[clone] bootstrap send failed: {e}");
    std::process::exit(CloneExitSend);
  }
  drop(bootstrapTx);

  let cache: &mut FxHashMap<String, Library> =
    Box::leak(Box::new(FxHashMap::default()));
  CloneSide { requestRx, responseTx }.run(cache)
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
