use crate::worker::takeLastErrno;
use crate::ffi::types::{Type, Value};
use crate::ffi::errors::FFIError;
use crate::worker::executeFFI;
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
// =================================================================================================

/* todo
    There are several possible directions for improvements and experiments:
    1. Pair work. The idea is simple - there is 1 zygote for cloning and 2 of its clones.
       Essentially, this is a pool of cloned zygotes. So there would be 3 of them in total.
       Basically, while one is working, another one is ready to take the hit right after it.
       This should significantly reduce the load in tasks where FFIs go one after another.
       (Кстати, для перезапуска main зиготы это очень важно - можно не создавать новую грязную,
       а если 1 умерла - можно клонировать через fork() быстро дубликат для дублирования).
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
  /// Successful execution with the returned value, plus `errno` captured
  /// immediately after the call — `Some` only if the request asked for it
  /// via `readErrno`, `None` otherwise (including for non-Call requests).
  Ok(Value, Option<i32>),

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
  /// Clone is ready.
  ///
  /// - `requestTx` — Runtime sends [`FFIRequest`] to the clone
  /// - `responseRx` — Runtime receives [`FFIResponse`] from the clone
  Clone {
    pid: i32,
    requestTx: IpcSender<FFIRequest>,
    responseRx: IpcReceiver<FFIResponse>
  },
  /// `ipc::channel()` or `fork()` failed inside Main Zygote.
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
#[derive(Serialize, Deserialize)]
struct CloneBootstrap
{
  requestTx: IpcSender<FFIRequest>,
  responseRx: IpcReceiver<FFIResponse>
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
  pub pid: libc::pid_t,
  /// Sends FFI requests to the clone.
  requestTx: IpcSender<FFIRequest>,
  /// Receives FFI responses from the clone.
  responseRx: IpcReceiver<FFIResponse>
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
      ZygoteReply::Clone {
        pid,
        requestTx,
        responseRx
      } =>
      {
        // 0 — explicit failure signal (should not appear if SpawnFailed is used,
        // but keep the guard for protocol robustness).
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
      ZygoteReply::SpawnFailed => Err(
        io::Error::other("Main zygote failed to create a clone (channel/fork failed)")
      )
    }
    //
  }

  /// FFI call inside a specific clone.
  pub(super) fn call(&self, request: FFIRequest) -> Result<FFIResponse, String>
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
}

impl Drop for ClonedZygote
{
  /// When drop() is called, the clone is immediately killed,
  /// the main zygote is not affected.
  fn drop(&mut self) -> ()
  {
    unsafe{ libc::kill(self.pid, libc::SIGKILL); }
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
  unsafe{ libc::signal(libc::SIGCHLD, libc::SIG_IGN); }

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
        // IMPORTANT (macOS / Mach ports):
        // Do NOT create data channels before fork(). Mach port rights are not
        // safely inherited across fork — Runtime then gets "Bogus destination port"
        // on the first send. Pattern used by ipc-channel itself:
        //   1. Parent creates a one-shot server (bootstrap name only).
        //   2. fork().
        //   3. Child creates channels AFTER fork, connects to the one-shot,
        //      sends Runtime-facing ends to the parent.
        //   4. Parent accept()s them and forwards to Runtime via ZygoteReply.
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

        // Forks the main zygote to spawn a fresh, clean clone process.
        match unsafe{ libc::fork() }
        {
          -1 =>
          {
            let _ = replyTx.send(ZygoteReply::SpawnFailed);
          }
          0 =>
          {
            // macOS / Mach ports + fork:
            // After fork the child inherits ipc-channel ends (cloneServer, commandRx,
            // replyTx) whose port names are often already invalid in the child task.
            // Calling Drop on them panics inside ipc-channel (InvalidName /
            // mach_port_deallocate). Parent still holds the live rights — child must
            // NOT Drop any pre-fork ipc-channel object; only mem::forget.
            std::mem::forget(cloneServer);
            std::mem::forget(commandRx);
            std::mem::forget(replyTx);
            // Channels are born here — after fork — so Mach rights are fresh.
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
            let bootstrapTx: IpcSender<CloneBootstrap> =
              match IpcSender::connect(cloneServerName)
              {
                Ok(tx) => tx,
                Err(_) => std::process::exit(1)
              };
            // Hand parent (→ Runtime) the ends that send requests / receive responses.
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
          pid =>
          { // Parent: receive Runtime-facing ends from the child, forward to Runtime.
            let (_rx, bootstrap): (IpcReceiver<CloneBootstrap>, CloneBootstrap) =
              match cloneServer.accept()
              {
                Ok(v) => v,
                Err(_) =>
                {
                  let _ = replyTx.send(ZygoteReply::SpawnFailed);
                  continue;
                }
              };
            let _ = replyTx.send(ZygoteReply::Clone {
              pid,
              requestTx: bootstrap.requestTx,
              responseRx: bootstrap.responseRx
            });
            //
          }
        }
        //
      }
    }
    //
  }
}

// =================================================================================================

/// Personal clone loop.
///
/// Waits for [`FFIRequest`] from Runtime, runs the operation, sends [`FFIResponse`].
fn cloneLoop(requestRx: IpcReceiver<FFIRequest>, responseTx: IpcSender<FFIResponse>) -> !
{
  // Local cache for dynamically loaded libraries to avoid costly repeated dlopen calls.
  let mut libraryCache: FxHashMap<String, Library> = FxHashMap::default();

  loop
  {
    // Wait for and read the incoming FFI request from the main runtime.
    let request: FFIRequest = match requestRx.recv()
    {
      Ok(r) => r,
      // Channel dropped — Runtime closed the clone handle / process exiting.
      Err(_) => std::process::exit(0)
    };

    // Execute the requested FFI operation using the cache and prepare the response.
    let response: FFIResponse = handleRequest(request, &mut libraryCache);

    // Transmit the execution result or error back to the parent process via IPC.
    if responseTx.send(response).is_err()
    {
      std::process::exit(0);
    }
  }
}

/// Handles an incoming request and performs an FFI operation using the library cache.
///
/// Returns the execution result or an error description.
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
    Ok(v) => FFIResponse::Ok(v, takeLastErrno()),
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
    unsafe{ libc::waitpid(pidToWait as libc::pid_t, std::ptr::null_mut(), 0); }

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
