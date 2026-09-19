//! Windows backend for [`super::Transport`].
//!
//! "Already good" backend from PR #53 — true process cloning via
//! `RtlCloneUserProcess` and named pipes for the data plane (handles cannot
//! survive `RtlCloneUserProcess`, so `ipc-channel` is only used to hand
//! over pipe names through the control channel).
//!
//! - **Control plane** (Runtime ↔ Main Zygote): one `IpcOneShotServer` set
//!   up in the Runtime; Main Zygote connects with `IpcSender::connect(name)`,
//!   hands over its `IpcSender<ZygoteCommand>` + `IpcReceiver<ZygoteReply>`,
//!   and the one-shot is dropped.
//!
//! - **Data plane** (Runtime ↔ Clone): per-clone named pipe created inside
//!   the freshly cloned process before advertising its name. Runtime opens
//!   the pipe by name.
// =================================================================================================
use super::Transport as TransportTrait;
use super::{
  CloneSide as CloneSideTrait, FFIRequest, FFIResponse,
  RuntimeSide as RuntimeSideTrait, ZygoteHandleBase
};
use crate::platform::low;
use crate::worker::executeFFI;
use crate::worker::{takeLastErrno, takeLastOsError};
use crate::zygote::ZygoteFlag;
use bincode::config::Configuration;
use fxhash::FxHashMap;
use ipc_channel::ipc::{self, IpcOneShotServer, IpcReceiver, IpcSender};
use libloading::Library;
use serde::{Deserialize, Serialize};
use std::env;
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
// =================================================================================================

/// Hidden startup flag of a legacy Command-based clone (kept for
/// compatibility — current path is `RtlCloneUserProcess`).
pub const CloneFlag: &str = "__zygoteClone";

/// Backend tag used in diagnostics.
const BackendName: &str = "windows-rtlcloneuserprocess";

// =================================================================================================

/// Commands Runtime → Main Zygote.
#[derive(Serialize, Deserialize)]
pub enum ZygoteCommand
{
  /// Ask Main Zygote to `RtlCloneUserProcess` a clone and return its pipe name.
  SpawnClone
}

/// Replies Main Zygote → Runtime.
#[derive(Serialize, Deserialize)]
pub enum ZygoteReply
{
  /// Clone is ready: named-pipe *name* only — no handle OOB.
  /// Runtime connects with `CreateFile`; clone already listens.
  Clone { pid: u32, dataPipe: String },
  
  /// `ipc::channel()` or `cloneProcess()` failed inside Main Zygote.
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

/// First message from a freshly cloned process → Main Zygote.
#[derive(Serialize, Deserialize)]
struct CloneBootstrap
{
  /// todo desc
  dataPipe: String
}

// =================================================================================================

/// Windows Transport: `RtlCloneUserProcess` + named pipes.
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
  /// Connected end of the clone's named pipe.
  pub dataPipe: low::Handle
}

// SAFETY: a Windows `HANDLE` is a kernel object handle owned by this
// process. The pipe is single-instance per clone (the Runtime connects to
// exactly one clone pipe at a time and tears it down before opening the
// next), so there is no concurrent aliasing to race against. The Send
// requirement comes from `ClonedZygote` being moved into `ZygoteStack`
// (thread-local) and read by `ClonedZygote::call` from the same thread
// that owns the rest of the call site.
unsafe impl Send for RuntimeSide {}

/// Clone-side data endpoint.
pub struct CloneSide
{
  /// Accepted end of the clone's named pipe.
  pub dataPipe: low::Handle
}

// SAFETY: see `RuntimeSide`. The clone is single-threaded by construction
// (only `cloneLoop` reads/writes the pipe), so transferring the handle to
// the call site (still this process) is sound.
unsafe impl Send for CloneSide {}

/// Bootstrap carried through the control channel from a freshly cloned
/// process back to the Runtime.
#[derive(Serialize, Deserialize)]
pub struct Bootstrap
{
  /// todo desc
  pub pid: u32,

  /// todo desc
  pub dataPipe: String
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

  /// Runtime asks Main Zygote to clone a process.
  fn sendSpawnClone(handle: &Self::ZygoteHandle) -> io::Result<Self::Bootstrap>
  {
    handle.commandTx.send(ZygoteCommand::SpawnClone).map_err(|e| {
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

    match reply
    {
      ZygoteReply::Clone { pid, dataPipe } => Ok(Bootstrap { pid, dataPipe }),
      ZygoteReply::SpawnFailed => Err(io::Error::other(
        "Main zygote failed to create a clone (channel/clone failed)"
      ))
    }
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
    let h: low::Handle = low::connectPipeClient(&bootstrap.dataPipe).ok_or_else(
      || io::Error::other("connect data pipe failed")
    )?;
    Ok(RuntimeSide { dataPipe: h })
  }
}

// =================================================================================================

impl RuntimeSideTrait for RuntimeSide
{
  /// todo desc
  fn send(&self, request: &FFIRequest) -> Result<(), String>
  {
    let config: Configuration = bincode::config::standard();
    let bytes: Vec<u8> = bincode::serde::encode_to_vec(request, config)
      .map_err(|e| format!("serialize FFIRequest: {e}"))?;

    if !low::pipeSend(self.dataPipe, &bytes) {
      return Err(format!(
        "Zygote clone IPC failed while sending request: pipe write failed \
         (GetLastError={})",
        low::lastPipeError()
      ));
    }
    Ok(())
  }

  /// todo desc
  fn recv(&self) -> Result<FFIResponse, String>
  {
    let config: Configuration = bincode::config::standard();
    let responseBytes: Vec<u8> = low::pipeRecv(self.dataPipe).ok_or_else(|| {
      format!(
        "Zygote clone IPC failed while reading response: pipe read failed \
         (GetLastError={})",
        low::lastPipeError()
      )
    })?;
    let (resp, _) = bincode::serde::decode_from_slice(&responseBytes, config)
      .map_err(|e| format!("deserialize FFIResponse: {e}"))?;
    Ok(resp)
  }
}

impl CloneSideTrait for CloneSide
{
  /// Dispatched through the trait, so Clippy sees it as "never used" —
  /// silenced here.
  #[allow(dead_code)]
  fn run(self, cache: &mut FxHashMap<String, Library>) -> !
  {
    let Self { dataPipe } = self;
    let mut libraryCache: FxHashMap<String, Library> = std::mem::take(cache);
    let cfg: Configuration = bincode::config::standard();

    loop
    {
      let bytes: Vec<u8> = match low::pipeRecv(dataPipe) {
        Some(b) => b,
        None => std::process::exit(0)
      };
      let (request, _): (FFIRequest, usize) =
        match bincode::serde::decode_from_slice(&bytes, cfg) {
          Ok(v) => v,
          Err(_) => std::process::exit(1)
        };

      // catch_unwind: a panic inside the clone must not abort the process
      // (ERROR_BROKEN_PIPE on the Runtime side). Convert to
      // FFIResponse::Err. Note: true AVs / SEH still kill the process —
      // that is intentional isolation.
      let response: FFIResponse =
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
          handleRequest(request, &mut libraryCache)
        })) {
          Ok(r) => r,
          Err(_) => crate::ffi::errors::FFIError::Other(
            "clone panicked while handling request".into()
          )
          .pipeErr()
        };

      let out = match bincode::serde::encode_to_vec(&response, cfg) {
        Ok(v) => v,
        Err(_) => std::process::exit(1)
      };
      if !low::pipeSend(dataPipe, &out) {
        std::process::exit(0);
      }
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
      ZygoteCommand::SpawnClone =>
      {
        let (cloneServer, cloneServerName): (
          IpcOneShotServer<CloneBootstrap>,
          String
        ) = match IpcOneShotServer::new() {
          Ok(s) => s,
          Err(_) => {
            let _ = replyTx.send(ZygoteReply::SpawnFailed);
            continue;
          }
        };

        let spawned: Option<u32> = match low::cloneProcess() {
          Ok(result) => {
            let pid: low::ProcessId = result.pid;
            // Thread already running (no CREATE_SUSPENDED).
            low::closeCloneHandles(&result);
            Some(pid)
          }
          Err(low::StatusProcessCloned) => {
            std::mem::forget(cloneServer);
            std::mem::forget(commandRx);
            std::mem::forget(replyTx);

            // We are the clone. The inherited ntdll CSR data block
            // (CsrPortHandle, CsrInitOnceDone, CsrPortHeap, CsrHeap, ...)
            // references the parent's CSR_PROCESS on the csrss.exe side.
            // Any Win32/basesrv call (reattachConsole, _stat64 in
            // handleRequest, etc.) AVs and we die with ERROR_BROKEN_PIPE
            // (109) on the data pipe. reconnectCsr() zeroes the whole
            // block, calls CsrClientConnectToServer for BASESRV + USERSRV
            // against \Sessions\{sid}\Windows, and registers the current
            // thread with RtlRegisterThreadWithCsrss. Best-effort: if
            // symbols were never resolved, we proceed anyway — same
            // failure mode as before this fix.
            let csrOk: bool = low::reconnectCsr();

            // reattachConsole goes through Win32 → CSRSS. If CSR was
            // not reconnected (ARM64 without a resolved block), the
            // stale ALPC port makes FreeConsole/AttachConsole hang —
            // the clone never reaches cloneBootstrapLoop, and the
            // parent blocks forever on cloneServer.accept().
            if csrOk {
              low::reattachConsole();
            }
            low::silenceCrashReporting();
            cloneBootstrapLoop(cloneServerName)
          }
          Err(_) => None
        };

        let Some(pid) = spawned else {
          let _ = replyTx.send(ZygoteReply::SpawnFailed);
          continue;
        };

        let (_rx, bootstrap): (
          IpcReceiver<CloneBootstrap>,
          CloneBootstrap
        ) = match cloneServer.accept() {
          Ok(v) => v,
          Err(_) => {
            low::killProcess(pid);
            let _ = replyTx.send(ZygoteReply::SpawnFailed);
            continue;
          }
        };

        let _ = replyTx.send(ZygoteReply::Clone {
          pid,
          dataPipe: bootstrap.dataPipe
        });
      }
    }
  }
}

/// Bootstrap loop in a freshly cloned process.
fn cloneBootstrapLoop(serverName: String) -> !
{
  let myPid: u32 = low::currentProcessId();
  let pipeName: String = low::cloneDataPipeName(myPid);

  // Create the duplex server BEFORE advertising the name.
  let dataPipe: low::Handle = match low::createPipeServer(&pipeName) {
    Some(h) => h,
    None => std::process::exit(1)
  };

  let bootstrapTx: IpcSender<CloneBootstrap> =
    match IpcSender::connect(serverName) {
      Ok(tx) => tx,
      Err(_) => std::process::exit(1)
    };
  if bootstrapTx.send(CloneBootstrap { dataPipe: pipeName }).is_err() {
    std::process::exit(1);
  }
  drop(bootstrapTx);

  // Block until Runtime connects.
  if !low::acceptPipeClient(dataPipe) {
    std::process::exit(1);
  }

  let cache: &mut FxHashMap<String, Library> =
    Box::leak(Box::new(FxHashMap::default()));
  CloneSide { dataPipe }.run(cache)
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

trait PipeErr
{
  /// todo desc
  fn pipeErr(self) -> FFIResponse;
}

impl PipeErr for crate::ffi::errors::FFIError
{
  /// todo desc
  fn pipeErr(self) -> FFIResponse
  {
    FFIResponse::Err(self)
  }
}

// =================================================================================================