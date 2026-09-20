//! Linux backend for [`super::Transport`].
//!
//! `ipc-channel` on both planes (`SOCK_SEQPACKET` sockets and `SCM_RIGHTS`
//! under the hood). Unlike macOS, `fork` needs no workaround here: file
//! descriptors are inherited by the child as they are, so the data channels
//! are created in Main Zygote *before* the fork and the clone simply keeps
//! its ends.
//!
//! - **Control plane** (Runtime ↔ Main Zygote): one `IpcOneShotServer` set up
//!   in the Runtime; Main Zygote connects with `IpcSender::connect(name)`,
//!   hands over its own `IpcSender<ZygoteCommand>` + `IpcReceiver<ZygoteReply>`,
//!   and the one-shot is dropped. Main Zygote is started with
//!   `Command::new(...)` (fork **and** exec) — a direct `fork()` from a
//!   warmed-up multithreaded Runtime would inherit locked mutexes, `exec()`
//!   wipes that state.
//!
//! - **Data plane** (Runtime ↔ Clone): two `ipc::channel()` pairs per clone,
//!   created inside Main Zygote just before `libc::fork()`. The clone keeps
//!   the clone-side ends (`requestRx`, `responseTx`) through the `fork` (no
//!   handoff needed); the Runtime-side ends (`requestTx`, `responseRx`) are
//!   forwarded to the Runtime over the control channel in
//!   [`ZygoteReply::Clone`].
// =================================================================================================
use super::Transport as TransportTrait;
use super::{
  CloneSide as CloneSideTrait, FFIRequest, FFIResponse,
  RuntimeSide as RuntimeSideTrait, ZygoteFlag, ZygoteHandleBase
};
use crate::platform::low;
use crate::worker::executeFFI;
use crate::worker::{takeLastErrno, takeLastOsError};
use fxhash::FxHashMap;
use ipc_channel::ipc::{self, IpcOneShotServer, IpcReceiver, IpcSender};
use libloading::Library;
use serde::{Deserialize, Serialize};
use std::env;
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
// =================================================================================================

/// Backend tag used in diagnostics.
const BackendName: &str = "linux-ipc-channel";

// =================================================================================================

/// Commands Runtime → Main Zygote.
#[derive(Serialize, Deserialize)]
pub enum ZygoteCommand
{
  /// Ask Main Zygote to `fork` a clone and return IPC endpoints to it.
  SpawnClone
}

/// Replies Main Zygote → Runtime.
///
/// `IpcSender` / `IpcReceiver` are transferable over ipc-channel themselves
/// (no manual `sendmsg` / SCM_RIGHTS).
#[derive(Serialize, Deserialize)]
pub enum ZygoteReply
{
  /// Clone is ready: transferable ipc-channel ends.
  Clone {
    pid: u32,
    requestTx: IpcSender<FFIRequest>,
    responseRx: IpcReceiver<FFIResponse>
  },

  /// `ipc::channel()` or `fork()` failed inside Main Zygote.
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

// =================================================================================================

/// Linux Transport: ipc-channel.
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

/// Bootstrap carried through the control channel from Main Zygote back to
/// the Runtime.
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

    //
    let currentExe: PathBuf = env::current_exe()?;
    // todo Might fail if the path to the executable file
    //  is too long or there are no permissions?
    let process: Child = Command::new(currentExe)
      .arg(ZygoteFlag)
      .arg(&serverName)
      .stdin(Stdio::null())
      .stdout(Stdio::inherit())
      .stderr(Stdio::inherit())
      .spawn()?;

    // Zygote connects, sends BootstrapToRuntime { commandTx, replyRx }.
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

  /// Runtime asks Main Zygote to fork a clone and returns its bootstrap.
  fn sendSpawnClone(handle: &Self::ZygoteHandle) -> io::Result<Self::Bootstrap>
  {
    handle
      .commandTx
      .send(ZygoteCommand::SpawnClone)
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

    match reply
    {
      ZygoteReply::Clone { pid, requestTx, responseRx } => Ok(Bootstrap {
        pid,
        requestTx,
        responseRx
      }),
      ZygoteReply::SpawnFailed => Err(io::Error::other(
        "Main zygote failed to create a clone (channel/fork failed)"
      ))
    }
  }

  /// todo desc
  fn bootstrapPid(bootstrap: &Self::Bootstrap) -> u32
  {
    bootstrap.pid
  }

  /// Enters the Main Zygote command loop. Called inside the freshly spawned
  /// Zygote (before any clone exists). Never returns.
  ///
  /// `flag`: the `IpcOneShotServer` name passed as `argv[2]`.
  ///
  /// There is no `cloneEnter` on Linux: `fork()` hands the clone its ends of
  /// the data-plane channels as ordinary variables, and the child runs the
  /// request loop right in [`zygoteLoop`].
  fn zygoteControlLoop(flag: Option<String>) -> !
  {
    let serverName: String =
      flag.expect("linux::zygoteControlLoop: missing IpcOneShotServer name");
    zygoteLoop(serverName)
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
  /// Runs the per-clone request/response loop until the Runtime closes its
  /// ends or a fatal error occurs. Never returns.
  ///
  /// Any I/O error means the Runtime closed the channel (or the clone died) —
  /// the clone `std::process::exit(0)`s and the kernel reaps it (because
  /// `SIGCHLD` is ignored in Main Zygote).
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

      let response: FFIResponse = handleRequest(request, &mut libraryCache);

      if responseTx.send(response).is_err() {
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

/// Main zygote loop: an infinite command waiting loop.
/// Which FFI will be needed is unknown in advance.
///
/// The zygote is an empty runtime template;
/// `dlopen` only works with the forked zygote.
fn zygoteLoop(serverName: String) -> !
{
  low::ignoreChildExits();

  // Control channels: Runtime holds commandTx + replyRx;
  // Main Zygote holds commandRx + replyTx.
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

  // Connect to Runtime's one-shot server and hand over the ends Runtime needs.
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
      Err(_) => std::process::exit(0) // Runtime / control channel died
    };

    match cmd
    {
      ZygoteCommand::SpawnClone =>
      {
        // The data channels of the new clone, before the fork: the child
        // inherits every descriptor as it is.
        let (requestTx, requestRx): (
          IpcSender<FFIRequest>,
          IpcReceiver<FFIRequest>
        ) = match ipc::channel::<FFIRequest>() {
          Ok(p) => p,
          Err(_) => {
            let _ = replyTx.send(ZygoteReply::SpawnFailed);
            continue;
          }
        };
        let (responseTx, responseRx): (
          IpcSender<FFIResponse>,
          IpcReceiver<FFIResponse>
        ) = match ipc::channel::<FFIResponse>() {
          Ok(p) => p,
          Err(_) => {
            let _ = replyTx.send(ZygoteReply::SpawnFailed);
            continue;
          }
        };

        match unsafe{ libc::fork() }
        {
          -1 =>
          { // Fork failed — the channels are dropped with this scope.
            let _ = replyTx.send(ZygoteReply::SpawnFailed);
          }
          0 =>
          { // Zygote clone: close the Runtime ends of the data plane and our
            // (inherited) control plane, and enter the loop. The control
            // plane is parent-only; clones don't speak it. Closing our
            // copies also lets the Runtime see EOF if Main Zygote dies while
            // clones are alive.
            drop(requestTx);
            drop(responseRx);
            drop(commandRx);
            drop(replyTx);

            let cache: &mut FxHashMap<String, Library> =
              Box::leak(Box::new(FxHashMap::default()));
            CloneSide { requestRx, responseTx }.run(cache)
          }
          pid =>
          { // Main zygote: close the clone ends of the data plane, forward
            // the Runtime ends together with the PID over the control plane.
            drop(requestRx);
            drop(responseTx);
            let _ = replyTx.send(ZygoteReply::Clone {
              pid: pid as u32,
              requestTx,
              responseRx
            });
          }
        }
      }
    }
  }
}

// =================================================================================================
