use crate::ffi::errors::FFIError;
use crate::ffi::types::{Type, Value};
use crate::sys;
use crate::worker::executeFFI;
use crate::worker::{takeLastErrno, takeLastOsError};
use fxhash::FxHashMap;
use ipc_channel::ipc::{self, IpcOneShotServer, IpcReceiver, IpcSender};
use libloading::Library;
use parking_lot::{Mutex, MutexGuard};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::env;
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::thread;
#[cfg(windows)]
use bincode::config::Configuration;
#[cfg(windows)]
use crate::sys::{Handle, ProcessId};
// =================================================================================================

/* todo
    There are several possible directions for improvements and experiments:
    1. Pair work. The idea is simple - there is 1 zygote for cloning and 2 of its clones.
       Essentially, this is a pool of cloned zygotes. So there would be 3 of them in total.
       Basically, while one is working, another one is ready to take the hit right after it.
       This should significantly reduce the load in tasks where FFIs go one after another.
    1.1.
       (By the way, for restarting the main zygote this is very important — 
       you don't have to create a new dirty one, and if one died — you can quickly clone 
       a duplicate through fork() for duplication).
    2. Dynamic zygote warming. The idea is also simple - depending on the load,
       we increase or decrease the number of cloned zygotes.
       This can be done using different algorithms.
    3. Splitting the Runtime into 2 parts - where the main zygote as a process initially
       will not even see the main Runtime. Something like 2 programs inside one.
       But making 2 programs is not a great approach - there needs to be a single file.
       This can be implemented in different ways. The idea is simple -
       even if the main zygote does not use Runtime instructions,
       it still declares them and they exist inside,
       although they will never be used.
       In some way, exec() and ctor solve this, but this solution fully solves it.
*/

// =================================================================================================

/// Hidden startup flag: if it is the first argument —
/// this is not the runtime, but the zygote process.
pub const ZygoteFlag: &str = "__zygote";

/// Hidden startup flag of a clone on platforms without `fork` (Windows):
/// a clone there is a fresh process re-running this executable, and this
/// flag is what tells it to become a clone instead of an ordinary Runtime.
#[cfg(windows)]
pub const CloneFlag: &str = "__zygoteClone";

/// Request for FFI execution, sent entirely to the zygote.
#[derive(Debug, Serialize, Deserialize)]
pub enum FFIRequest
{
  /// Calls a function from a dynamic library with the given arguments and expected return type.
  ///
  /// `readErrno`: when true, the clone reads `errno` immediately after the C call
  /// returns and reports it back via [`FFIResponse::Ok`]'s second field. Costs one
  /// extra read when set — calls that don't need it can leave it `false`.
  Call {
    libraryPath: String,
    functionName: String,
    args: Vec<Value>,
    resultType: Type,
    readErrno: bool
  },

  /// Allocates a block of memory of the specified length in the zygote address space.
  Alloc { length: usize },
  /// Allocates enough memory to hold a dynamically-shaped struct — the byte
  /// size (with correct platform padding/alignment) is computed by `libffi`
  /// on the clone side from `fields`, not guessed by the caller. Response
  /// carries both the pointer and the resolved size (see `executeFFI`).
  AllocDynamicStruct { fields: Vec<Type> },
  /// Allocates a block of memory with specific alignment.
  /// Uses `posix_memalign` under the hood — alignment must be a power of 2.
  AllocAligned { length: usize, alignment: usize },
  /// Frees a previously allocated memory block by its pointer.
  Free { pointer: usize },

  /// Reads a raw memory block of the given length starting at the specified pointer.
  ReadMemory { pointer: usize, length: usize },
  /// Writes a value to the specified address in the zygote memory.
  WriteMemory { pointer: usize, value: Value },

  /// Reads a dynamically-typed struct at `pointer`. Field byte offsets
  /// (padding, alignment) are computed by `libffi` for the current ABI,
  /// not assumed — this is what makes [`Type::Struct`] usable for shapes
  /// that don't exist as a Rust type at compile time.
  ReadDynamicStruct { pointer: usize, fields: Vec<Type> },
  /// Writes `values` into a dynamically-typed struct at `pointer`.
  WriteDynamicStruct {
    pointer: usize,
    fields: Vec<Type>,
    values: Vec<Value>
  },

  /// Parent sends a serialized closure; the clone deserializes and stores it.
  RegisterCallback {
    id: u64,
    bytes: Vec<u8>,
    argTypes: Vec<Type>,
    returnType: Type
  },
  /// Calls a function directly by its raw memory pointer
  /// with the provided arguments and expected return type.
  ///
  /// `readErrno`: see [`FFIRequest::Call`].
  CallPointer {
    pointer: usize,
    args: Vec<Value>,
    resultType: Type,
    readErrno: bool
  },
}

/// Response to the request with the execution result or error.
#[derive(Serialize, Deserialize)]
pub enum FFIResponse
{
  /// Successful execution with the returned value, plus `errno` and — on
  /// Windows — `GetLastError`, both captured immediately after the call.
  /// `Some` only if the request asked for it via `readErrno`, `None`
  /// otherwise. The third field stays `None` outside Windows.
  Ok(Value, Option<i32>, Option<u32>),

  /// Execution failed with the corresponding error.
  Err(FFIError)
}

// =================================================================================================
// Control plane (Runtime ↔ Main Zygote) over ipc-channel.
//
// On macOS ipc-channel uses Mach ports; on Linux/BSD — Unix sockets.
// This replaces the previous UnixStream::pair + SCM_RIGHTS handoff, which
// raced under parallel cargo tests on Darwin (FD not connected to the clone).
// =================================================================================================

/// Commands Runtime → Main Zygote.
#[derive(Serialize, Deserialize)]
enum ZygoteCommand
{
  /// Ask Main Zygote to `fork` a clone and return IPC endpoints to it.
  SpawnClone
}

/// Replies Main Zygote → Runtime.
///
/// `IpcSender` / `IpcReceiver` are transferable over ipc-channel themselves
/// (no manual `sendmsg` / SCM_RIGHTS).
#[derive(Serialize, Deserialize)]
enum ZygoteReply
{
  /// Clone is ready (Unix / macOS): transferable ipc-channel ends.
  #[cfg(unix)]
  Clone {
    pid: u32,
    requestTx: IpcSender<FFIRequest>,
    responseRx: IpcReceiver<FFIResponse>
  },
  /// Clone is ready (Windows): named-pipe *names* only — no handle OOB.
  /// Runtime connects with CreateFile; clone already listens.
  #[cfg(windows)]
  Clone {
    pid: u32,
    dataPipe: String
  },
  /// `ipc::channel()` or `fork()`/`RtlCloneUserProcess` failed inside Main Zygote.
  SpawnFailed
}

/// First message from Zygote after connecting to Runtime's [`IpcOneShotServer`].
/// Hands over the control endpoints Runtime needs.
#[derive(Serialize, Deserialize)]
struct BootstrapToRuntime
{
  commandTx: IpcSender<ZygoteCommand>,
  replyRx: IpcReceiver<ZygoteReply>
}

/// First message from a freshly forked clone → Main Zygote (post-fork channel setup).
/// Carries the ends that Runtime will use; clone keeps the opposite ends locally.
///
/// Unix: real transferable ipc-channel ends.
/// Windows: only pipe names (handles cannot cross RtlCloneUserProcess via ipc-channel).
#[cfg(unix)]
#[derive(Serialize, Deserialize)]
struct CloneBootstrap
{
  requestTx: IpcSender<FFIRequest>,
  responseRx: IpcReceiver<FFIResponse>
}

#[cfg(windows)]
#[derive(Serialize, Deserialize)]
struct CloneBootstrap
{
  dataPipe: String
}

// =================================================================================================

/// Controls the zygote process and the communication channel with it.
pub struct ZygoteHandle
{
  /// Child zygote process.
  process: Child,
  /// Runtime → Main Zygote commands.
  commandTx: IpcSender<ZygoteCommand>,
  /// Main Zygote → Runtime replies.
  replyRx: IpcReceiver<ZygoteReply>
}

impl Drop for ZygoteHandle
{
  /// Terminates the zygote process.
  fn drop(&mut self) -> ()
  {
    let _ = self.process.kill();
  }
}

/// Global state of the active zygote with synchronized access.
pub static ZygoteState: OnceLock<Mutex<ZygoteHandle>> = OnceLock::new();

// =================================================================================================

/// RAII handle for a separate zygote clone process.
pub struct ClonedZygote
{
  /// Process PID.
  pub pid: u32,
  
  /// todo desc
  #[cfg(unix)]
  requestTx: IpcSender<FFIRequest>,
  /// todo desc
  #[cfg(unix)]
  responseRx: IpcReceiver<FFIResponse>,
  
  /// Windows: one duplex named-pipe for request/response framing.
  #[cfg(windows)]
  dataPipe: Handle
}

impl ClonedZygote
{
  /// Requests a clone from the main zygote and returns its RAII handle.
  pub fn getMeClone() -> io::Result<Self>
  {
    let mutex: &Mutex<ZygoteHandle> = ZygoteState.get().ok_or_else(|| {
      io::Error::new(io::ErrorKind::NotFound, "Zygote not initialized")
    })?;
    let guard: MutexGuard<ZygoteHandle> = mutex.lock();

    // Ask Main Zygote to fork a clone and return the IPC ends.
    guard
      .commandTx
      .send(ZygoteCommand::SpawnClone)
      .map_err(|e| {
        io::Error::new(
          io::ErrorKind::BrokenPipe,
          format!("SpawnClone send failed: {e}"),
        )
      })?;

    let reply: ZygoteReply = guard.replyRx.recv().map_err(|e| {
      io::Error::new(
        io::ErrorKind::BrokenPipe,
        format!("SpawnClone reply failed: {e}"),
      )
    })?;
    drop(guard);

    match reply
    {
      #[cfg(unix)]
      ZygoteReply::Clone {
        pid,
        requestTx,
        responseRx
      } =>
      {
        if pid == 0
        {
          return Err(
            io::Error::other("Main zygote failed to create a clone (pid=0)")
          );
        }
        Ok(Self {
          pid,
          requestTx,
          responseRx
        })
      }
      #[cfg(windows)]
      ZygoteReply::Clone {
        pid,
        dataPipe
      } =>
      {
        if pid == 0
        {
          return Err(
            io::Error::other("Main zygote failed to create a clone (pid=0)")
          );
        }
        let h: Handle = sys::connectPipeClient(&dataPipe).ok_or_else(|| {
          io::Error::other("connect data pipe failed")
        })?;
        Ok(Self {
          pid,
          dataPipe: h
        })
      }
      ZygoteReply::SpawnFailed => Err(
        io::Error::other("Main zygote failed to create a clone (channel/fork failed)")
      )
    }
    //
  }

  /// FFI call inside a specific clone.
  pub(super) fn call(&self, request: FFIRequest) -> Result<FFIResponse, String>
  {
    #[cfg(unix)]
    {
      self
        .requestTx
        .send(request)
        .map_err(|e| format!("Zygote clone IPC failed while sending request: {e}"))?;

      self
        .responseRx
        .recv()
        .map_err(|e| format!("Zygote clone IPC failed while reading response: {e}"))
    }
    #[cfg(windows)]
    {
      // todo desc
      let config: Configuration = bincode::config::standard();
      let bytes: Vec<u8> = bincode::serde::encode_to_vec(&request, config)
        .map_err(|e| format!("serialize FFIRequest: {e}"))?;
      
      // todo desc
      if !sys::pipeSend(self.dataPipe, &bytes) {
        return Err(
          "Zygote clone IPC failed while sending request: pipe write failed".into()
        );
      }
      
      // todo desc
      let responseBytes: Vec<u8> = sys::pipeRecv(self.dataPipe).ok_or_else(|| {
        format!(
          "Zygote clone IPC failed while reading response: pipe read failed (GetLastError={})",
          sys::lastPipeError()
        )
      })?;
      
      // todo desc
      let (resp, _) = bincode::serde::decode_from_slice(&responseBytes, config)
        .map_err(|e| format!("deserialize FFIResponse: {e}"))?;
      Ok(resp)
    }
  }
}

impl Drop for ClonedZygote
{
  /// When drop() is called, the clone is immediately killed,
  /// the main zygote is not affected.
  fn drop(&mut self) -> ()
  {
    sys::killProcess(self.pid);
    #[cfg(windows)]
    {
      sys::closeHandle(self.dataPipe);
    }
  }
}

// =================================================================================================

thread_local! {
  /// Zygote stack: each entry into ffi!{} puts a new zygote on top of the stack.
  pub(crate) static ZygoteStack: RefCell<Vec<ClonedZygote>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard of the active zygote in the current thread's stack.
pub struct ZygoteGuard;

impl ZygoteGuard
{
  /// Adds a zygote to the stack and returns a guard for its lifetime.
  pub fn enter(zygote: ClonedZygote) -> Self
  {
    ZygoteStack.with(|stack| {
      stack.borrow_mut().push(zygote);
    });
    Self
  }
}

impl Drop for ZygoteGuard
{
  /// Removes exactly this zygote from the stack,
  /// and Rust automatically calls its drop(), killing the process.
  fn drop(&mut self) -> ()
  {
    ZygoteStack.with(|stack| {
      stack.borrow_mut().pop();
    });
  }
}

// =================================================================================================

/// Entry point of the child Zygote process;
///
/// main() must call this as the first line if the first argument == [`ZygoteFlag`];
///
/// The process is spawned through Command (fork+exec) — runtime was not warmed up,
/// there are no extra tasks, there is no metadata heap. The library is not loaded in advance.
///
/// Second argument (`argv[2]`) is the [`IpcOneShotServer`] name used to bootstrap
/// the control channel with the Runtime.
pub fn runAsZygote() -> !
{
  let serverName: String = env::args()
    .nth(2)
    .expect("zygote: missing IpcOneShotServer name (argv[2])");
  zygoteLoop(serverName);
}

/// Zygote initialization; call once,
/// as the very first line of the normal main().
pub fn initZygote() -> io::Result<()>
{
  let handle: ZygoteHandle = spawnZygote()?;
  ZygoteState
    .set(Mutex::new(handle))
    .map_err(|_| {
      io::Error::new(io::ErrorKind::AlreadyExists, "Zygote already initialized")
    })?;
  thread::spawn(supervisorLoop);
  Ok(())
}

/// The zygote is spawned only through Command (fork+exec) at startup.
///
/// This is fundamental: a regular fork() from an already warmed-up multithreaded runtime
/// (the supervisor is a separate thread) would inherit other mutexes in a locked state —
/// which would create a deadlock trap.
///
/// exec() completely replaces the process image,
/// therefore the Zygote is always born clean, regardless of how "heavy"
/// the runtime has become by the time of startup.
///
/// Bootstrap uses [`IpcOneShotServer`] (Mach ports on macOS, Unix sockets elsewhere)
/// instead of `UnixStream::pair` + SCM_RIGHTS — that path was racy under parallel
/// tests on Darwin (Runtime received a dead FD while the clone was still waiting).
pub fn spawnZygote() -> io::Result<ZygoteHandle>
{
  // Creates a one-shot IPC server to establish initial control channel with the zygote.
  let (server, serverName): (IpcOneShotServer<BootstrapToRuntime>, String) =
    IpcOneShotServer::new().map_err(io::Error::other)?;

  // todo Might fail if the path to the executable file
  //  is too long or there are no permissions?
  let currentExe: PathBuf = env::current_exe()?;
  let process: Child = Command::new(currentExe)
    .arg(ZygoteFlag)
    .arg(&serverName)
    .stdin(Stdio::null())
    .stdout(Stdio::inherit())
    .stderr(Stdio::inherit())
    .spawn()?;

  // Zygote connects, sends BootstrapToRuntime { commandTx, replyRx }.
  // The one-shot receiver side is unused after the first message — control
  // continues on the embedded commandTx / replyRx pair.
  let (_rx, bootstrap): (IpcReceiver<BootstrapToRuntime>, BootstrapToRuntime) = server
    .accept()
    .map_err(|e| {
      io::Error::other(format!("zygote bootstrap accept: {e}"))
    })?;

  //
  Ok(ZygoteHandle {
    process,
    commandTx: bootstrap.commandTx,
    replyRx: bootstrap.replyRx
  })
}

/// Main zygote loop: an infinite command waiting loop.
/// Which FFI will be needed is unknown in advance.
///
/// The zygote is an empty runtime template;
/// dlopen only works with the forked zygote.
fn zygoteLoop(serverName: String) -> !
{
  // Important: Ignoring SIGCHLD is needed only in the main Zygote.
  //
  // This makes the OS kernel automatically clean up
  // its clones on termination (without zombies).
  //
  // This must not be written in the main runtime: there, waitpid
  // in supervisorLoop() tracks the Zygote process itself,
  // and with SIG_IGN it will fail with ECHILD and enter guaranteed CPU load.
  sys::ignoreChildExits(); // no-op on Windows: no SIGCHLD, no zombies

  // Resolve the ntdll CSR data block [CsrServerApiRoutine .. RtlpEnvironLookupTable)
  // and kernelbase!CtrlRoutine once, in the healthy zygote, BEFORE any clone is
  // spawned. Children inherit the cached addresses via CoW and use them in
  // `reconnectCsr()` to zero the stale block, call CsrClientConnectToServer for
  // BASESRV + USERSRV, and RtlRegisterThreadWithCsrss. No-op on Unix.
  #[cfg(windows)]
  sys::resolveCsrPortHandle();

  // Control channels: Runtime holds commandTx + replyRx;
  // Main Zygote holds commandRx + replyTx.
  let (commandTx, commandRx): (IpcSender<ZygoteCommand>, IpcReceiver<ZygoteCommand>) =
    match ipc::channel::<ZygoteCommand>()
    {
      Ok(p) => p,
      Err(_) => std::process::exit(1)
    };
  let (replyTx, replyRx): (IpcSender<ZygoteReply>, IpcReceiver<ZygoteReply>) =
    match ipc::channel::<ZygoteReply>()
    {
      Ok(p) => p,
      Err(_) => std::process::exit(1)
    };

  // Connect to Runtime's one-shot server and hand over the ends Runtime needs.
  let bootstrapTx: IpcSender<BootstrapToRuntime> = match IpcSender::connect(serverName)
  {
    Ok(tx) => tx,
    Err(_) => std::process::exit(1)
  };
  if bootstrapTx
    .send(BootstrapToRuntime {
      commandTx,
      replyRx
    })
    .is_err()
  {
    std::process::exit(1);
  }
  drop(bootstrapTx);

  // Infinite event loop processing control commands from the main Runtime.
  loop
  {
    // Blocks until a new command is received via the control channel.
    let cmd: ZygoteCommand = match commandRx.recv()
    {
      Ok(c) => c,
      Err(_) => std::process::exit(0) // Runtime / control channel died
    };

    // Routes and executes the received control command.
    match cmd
    {
      ZygoteCommand::SpawnClone =>
      {
        // Parent creates a one-shot server; only the *name* crosses to the child.
        // Data path is established AFTER fork/clone:
        //   Unix  — ipc-channel ends created in child, sent over one-shot
        //   Windows — named pipes by name (ipc-channel handle OOB breaks under
        //             RtlCloneUserProcess)
        let (cloneServer, cloneServerName): (
          IpcOneShotServer<CloneBootstrap>,
          String
        ) = match IpcOneShotServer::new()
        {
          Ok(s) => s,
          Err(_) =>
          {
            let _ = replyTx.send(ZygoteReply::SpawnFailed);
            continue;
          }
        };

        #[cfg(unix)]
        let spawned: Option<u32> = match unsafe { libc::fork() }
        {
          -1 => None,
          0 =>
          {
            std::mem::forget(cloneServer);
            std::mem::forget(commandRx);
            std::mem::forget(replyTx);
            cloneBootstrapLoop(cloneServerName)
          }
          pid => Some(pid as u32)
        };

        #[cfg(windows)]
        let spawned: Option<u32> = match sys::cloneProcess()
        {
          Ok(result) =>
          {
            let pid: ProcessId = result.pid;
            // Thread already running (no CREATE_SUSPENDED).
            sys::closeCloneHandles(&result);
            Some(pid)
          }
          Err(sys::StatusProcessCloned) =>
          {
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
            let csrOk: bool = sys::reconnectCsr();
            eprintln!("[child] reconnectCsr={}", csrOk);
            
            // reattachConsole goes through Win32 → CSRSS. If CSR was
            // not reconnected (ARM64 without a resolved block), the
            // stale ALPC port makes FreeConsole/AttachConsole hang —
            // the clone never reaches cloneBootstrapLoop, and the
            // parent blocks forever on cloneServer.accept().
            if csrOk {
              sys::reattachConsole();
            }
            sys::silenceCrashReporting();
            cloneBootstrapLoop(cloneServerName)
          }
          Err(_) => None
        };

        let Some(pid) = spawned else
        {
          let _ = replyTx.send(ZygoteReply::SpawnFailed);
          continue;
        };

        // Receive the Runtime-facing ends from the clone, forward them on.
        let (_rx, bootstrap): (IpcReceiver<CloneBootstrap>, CloneBootstrap) =
          match cloneServer.accept()
          {
            Ok(v) => v,
            Err(_) =>
            {
              sys::killProcess(pid);
              let _ = replyTx.send(ZygoteReply::SpawnFailed);
              continue;
            }
          };

        #[cfg(unix)]
        let _ = replyTx.send(ZygoteReply::Clone {
          pid,
          requestTx: bootstrap.requestTx,
          responseRx: bootstrap.responseRx
        });
        #[cfg(windows)]
        let _ = replyTx.send(ZygoteReply::Clone {
          pid,
          dataPipe: bootstrap.dataPipe
        });
      }
    }
    //
  }
}

// =================================================================================================

/// todo desc
fn cloneBootstrapLoop(serverName: String) -> !
{
  #[cfg(unix)]
  {
    let (requestTx, requestRx): (IpcSender<FFIRequest>, IpcReceiver<FFIRequest>) =
      match ipc::channel::<FFIRequest>()
      {
        Ok(p) => p,
        Err(_) => std::process::exit(1)
      };
    let (responseTx, responseRx): (IpcSender<FFIResponse>, IpcReceiver<FFIResponse>) =
      match ipc::channel::<FFIResponse>()
      {
        Ok(p) => p,
        Err(_) => std::process::exit(1)
      };

    let bootstrapTx: IpcSender<CloneBootstrap> = match IpcSender::connect(serverName)
    {
      Ok(tx) => tx,
      Err(_) => std::process::exit(1)
    };

    if bootstrapTx
      .send(CloneBootstrap {
        requestTx,
        responseRx
      })
      .is_err()
    {
      std::process::exit(1);
    }
    drop(bootstrapTx);

    cloneLoop(requestRx, responseTx);
  }

  #[cfg(windows)]
  {
    let myPid: u32 = sys::currentProcessId();
    let pipeName: String = sys::cloneDataPipeName(myPid);

    // Create the duplex server BEFORE advertising the name.
    let dataPipe: Handle = match sys::createPipeServer(&pipeName)
    {
      Some(h) => h,
      None => std::process::exit(1)
    };

    let bootstrapTx: IpcSender<CloneBootstrap> = match IpcSender::connect(serverName)
    {
      Ok(tx) => tx,
      Err(_) => std::process::exit(1)
    };
    if bootstrapTx
      .send(CloneBootstrap {
        dataPipe: pipeName
      })
      .is_err()
    {
      std::process::exit(1);
    }
    drop(bootstrapTx);

    // Block until Runtime connects.
    if !sys::acceptPipeClient(dataPipe)
    {
      std::process::exit(1);
    }

    cloneLoopWindows(dataPipe);
  }
}

/// Legacy Command-based clone entry (kept for compatibility).
#[cfg(windows)]
pub fn runAsClone() -> !
{
  let serverName: String = env::args()
    .nth(2)
    .expect("zygote clone: missing IpcOneShotServer name (argv[2])");

  sys::silenceCrashReporting();
  cloneBootstrapLoop(serverName);
}

// =================================================================================================

/// todo desc
#[cfg(unix)]
fn cloneLoop(requestRx: IpcReceiver<FFIRequest>, responseTx: IpcSender<FFIResponse>) -> !
{
  let mut libraryCache: FxHashMap<String, Library> = FxHashMap::default();

  loop
  {
    let request: FFIRequest = match requestRx.recv()
    {
      Ok(r) => r,
      Err(_) => std::process::exit(0)
    };

    let response: FFIResponse = handleRequest(request, &mut libraryCache);

    if responseTx.send(response).is_err()
    {
      std::process::exit(0);
    }
  }
}

/// todo desc
#[cfg(windows)]
fn cloneLoopWindows(dataPipe: Handle) -> !
{
  // todo desc
  let mut libraryCache: FxHashMap<String, Library> = FxHashMap::default();
  let cfg: Configuration = bincode::config::standard();

  // todo desc
  loop
  {
    // todo desc
    let bytes: Vec<u8> = match sys::pipeRecv(dataPipe)
    {
      Some(b) => b,
      None => std::process::exit(0)
    };
    let (request, _): (FFIRequest, usize) = match bincode::serde::decode_from_slice(&bytes, cfg)
    {
      Ok(v) => v,
      Err(_) => std::process::exit(1)
    };

    // catch_unwind: a panic inside the clone must not abort the process
    // (ERROR_BROKEN_PIPE on the Runtime side). Convert to FFIResponse::Err.
    // Note: true AVs / SEH still kill the process — that is intentional isolation.
    let response: FFIResponse = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      handleRequest(request, &mut libraryCache)
    }))
    {
      Ok(r) => r,
      Err(_) => FFIResponse::Err(FFIError::Other("clone panicked while handling request".into()))
    };

    // todo desc
    let out = match bincode::serde::encode_to_vec(&response, cfg)
    {
      Ok(v) => v,
      Err(_) => std::process::exit(1)
    };
    if !sys::pipeSend(dataPipe, &out)
    {
      std::process::exit(0);
    }
  }
}

// =================================================================================================

/// todo desc
fn handleRequest(
  request: FFIRequest,
  cache: &mut FxHashMap<String, Library>
) -> FFIResponse
{
  match executeFFI(request, cache)
  {
    // `takeLastErrno` reads whatever `invokeFFI` stashed right after `cif.call()`
    // (or `None`, for requests that never call — Alloc, Free, ReadMemory, ...
    // and for calls that didn't ask for it) — and clears it for the next request.
    Ok(v) => FFIResponse::Ok(v, takeLastErrno(), takeLastOsError()),
    Err(e) => FFIResponse::Err(e)
  }
}

// =================================================================================================

/// Supervisor: blocks on the death of the current zygote (waitpid) and recreates it.
///
/// Separate thread — therefore [`spawnZygote()`] inside must go through Command, not fork().
fn supervisorLoop() -> ()
{
  loop
  {
    // Retrieves the PID of the currently active main zygote for monitoring.
    let pidToWait: u32 = {
      let mutex: &Mutex<ZygoteHandle> = match ZygoteState.get()
      {
        Some(m) => m,
        None => return
      };
      mutex.lock().process.id()
    };

    // Blocks the supervisor thread until the monitored main zygote process terminates.
    sys::waitProcess(pidToWait);

    // Acquires the global state lock to replace the terminated zygote with a new instance.
    let mutex: &Mutex<ZygoteHandle> = ZygoteState.get().unwrap();
    let mut guard: MutexGuard<ZygoteHandle> = mutex.lock();
    if guard.process.id() == pidToWait // Not recreated in parallel yet through call()
    {
      match spawnZygote()
      {
        Ok(newHandle) =>
        {
          *guard = newHandle;
        }
        Err(_) =>
        {
          drop(guard);
          thread::sleep(std::time::Duration::from_millis(200));
        }
      }
    }
    //
  }
}

// =================================================================================================

#[cfg(test)]
mod tests
{
  use crate::ffi;
  use crate::ffi::errors::FFIError;
  use crate::platform::{platformExt, LibmPath};
  // ===============================================================================================

  /// The headline guarantee of this crate: a real SIGSEGV inside an
  /// isolated clone must surface as `Err`, never take down the process
  /// running this test. Uses `examples/isolation/crash.c`, built by
  /// `build.rs` for every `cargo build`/`cargo test`, not just `cargo run
  /// --example isolation`.
  #[test]
  fn segfaultIsIsolated() -> ()
  {
    let result: Result<(), FFIError> = ffi!(|scope| {
      scope.addSearchPath("examples/isolation");
      let lib: Library = scope.load(platformExt!("libcrash"))?;
      lib.call("triggerSegfault").void()
    });

    let err: FFIError = result.expect_err("a segfaulting clone must not report success");
    assert!(matches!(err, FFIError::ZygoteCommunicationFailed(_)), "unexpected error: {err:?}");
  }

  /// Same guarantee for `abort()` (SIGABRT) — a different signal, same boundary.
  #[test]
  fn abortIsIsolated() -> ()
  {
    let result: Result<(), FFIError> = ffi!(|scope| {
      scope.addSearchPath("examples/isolation");
      let lib: Library = scope.load(platformExt!("libcrash"))?;
      lib.call("triggerAbort").void()
    });

    let err: FFIError = result.expect_err("an aborting clone must not report success");
    assert!(matches!(err, FFIError::ZygoteCommunicationFailed(_)), "unexpected error: {err:?}");
  }

  /// A crashed clone must not affect the main zygote: a completely
  /// unrelated `ffi!` block right after still succeeds. Each `ffi!` forks
  /// its own fresh clone from the always-alive main zygote — a crashed
  /// clone never touches the main zygote itself, so this needs no
  /// supervisor-restart delay to be meaningful.
  #[test]
  fn runtimeSurvivesAfterCrash() -> ()
  {
    let crashed: Result<(), FFIError> = ffi!(|scope| {
      scope.addSearchPath("examples/isolation");
      let lib: Library = scope.load(platformExt!("libcrash"))?;
      lib.call("triggerAbort").void()
    });
    assert!(crashed.is_err(), "sanity check: the setup call should have crashed");

    let result: f64 = ffi!(|scope| {
      let libm: Library = scope.load(LibmPath)?;
      libm.call("sqrt").arg::<f64>(16.0).result()
    }).expect("runtime should survive a crashed clone");

    assert!((result - 4.0).abs() < f64::EPSILON);
  }

  // ===============================================================================================

  /// Repeated sequential `ffi!` blocks stress the control plane:
  /// each iteration does `SpawnClone` → IPC bootstrap → drop/kill.
  /// This is the minimal regression for the IPC race that previously
  /// appeared only under high test volume (many independent clones
  /// from one main zygote in a single process).
  #[test]
  fn sequentialCloneStress() -> ()
  {
    const Iterations: usize = 50;

    for i in 0..Iterations
    {
      let result: f64 = ffi!(|scope| {
        let libm: Library = scope.load(LibmPath)?;
        libm.call("sqrt").arg::<f64>(4.0).result()
      }).unwrap_or_else(|e| panic!("sequential clone stress failed on iteration {i}: {e}"));

      assert!((result - 2.0).abs() < f64::EPSILON, "unexpected sqrt result on iteration {i}");
    }
  }

  /// Concurrent `ffi!` from several threads is the scenario that
  /// exposed the original Darwin IPC bug (dead FD / "Bogus destination
  /// port" under parallel `SpawnClone`). Channels are created after
  /// `fork` and handed over via `IpcOneShotServer`; this test keeps
  /// pressure on that path so a regression cannot hide behind
  /// sequential-only runs.
  #[test]
  fn concurrentCloneStress() -> ()
  {
    use std::thread;

    const Threads: usize = 8;
    const PerThread: usize = 20;

    let handles: Vec<thread::JoinHandle<()>> = (0..Threads)
      .map(|t| {
        thread::spawn(move || {
          for i in 0..PerThread
          {
            let result: f64 = ffi!(|scope| {
              let libm: Library = scope.load(LibmPath)?;
              libm.call("sqrt").arg::<f64>(4.0).result()
            }).unwrap_or_else(|e| {
              panic!("concurrent clone stress failed on thread {t} iteration {i}: {e}")
            });

            assert!(
              (result - 2.0).abs() < f64::EPSILON,
              "unexpected sqrt result on thread {t} iteration {i}"
            );
          }
        })
      })
      .collect();

    for handle in handles
    {
      handle.join().expect("concurrent clone stress thread panicked");
    }
  }

  /// Pure create/drop of clones without any FFI call.
  /// Isolates the control-plane path (`SpawnClone` + channel
  /// bootstrap + kill) from library loading and `libffi` work.
  #[test]
  fn rapidCloneCreateDrop() -> ()
  {
    const Iterations: usize = 100;

    for i in 0..Iterations
    {
      let zygote: crate::zygote::ClonedZygote = crate::zygote::ClonedZygote::getMeClone()
        .unwrap_or_else(|e| panic!("getMeClone failed on iteration {i}: {e}"));
      drop(zygote);
    }
  }

  // ===============================================================================================
}

// =================================================================================================