use parking_lot::MutexGuard;
use parking_lot::Mutex;
use std::sync::OnceLock;
use crate::ffi::errors::FFIError;
use std::cell::RefCell;
use std::env;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixStream};
use std::os::fd::{AsRawFd, FromRawFd, RawFd, OwnedFd};
use std::path::PathBuf;
use std::process::{Command, Child, Stdio};
use std::thread;
use bincode::config::Configuration;
use fxhash::FxHashMap;
use libloading::Library;
use serde::{Serialize, Deserialize};
use crate::ffi::value::{Type, Value};
use crate::worker::executeFFI;
// =================================================================================================

/* todo
    There are several possible directions for improvements and experiments:
    1. Pair work. The idea is simple - there is 1 zygote for cloning and 2 of its clones.
       Essentially, this is a pool of cloned zygotes. So there would be 3 of them in total.
       Basically, while one is working, another one is ready to take the hit right after it.
       This should significantly reduce the load in tasks where FFIs go one after another.
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
pub(super) const ZygoteFlag: &str = "__zygote";

/// Request for FFI execution, sent entirely to the zygote.
#[derive(Debug, Serialize, Deserialize)]
pub(super) enum FFIRequest
{
  /// Calls a function from a dynamic library with the given arguments and expected return type.
  Call { libraryPath: String, functionName: String, args: Vec<Value>, resultType: Type },

  /// Allocates a block of memory of the specified length in the zygote address space.
  Alloc { length: usize },
  /// Frees a previously allocated memory block by its pointer.
  Free { pointer: usize },

  /// Reads a raw memory block of the given length starting at the specified pointer.
  ReadMemory { pointer: usize, length: usize },
  /// Writes a value to the specified address in the zygote memory.
  WriteMemory { pointer: usize, value: Value },

  /// Reads a dynamically-typed struct at `pointer`. Field byte offsets
  /// (padding, alignment) are computed by libffi for the current ABI,
  /// not assumed — this is what makes `Type::Struct` usable for shapes
  /// that don't exist as a Rust type at compile time.
  ReadDynamicStruct { pointer: usize, fields: Vec<Type> },
  /// Writes `values` into a dynamically-typed struct at `pointer`.
  WriteDynamicStruct { pointer: usize, fields: Vec<Type>, values: Vec<Value> },

  /// Parent sends a serialized closure; the clone deserializes and stores it.
  RegisterCallback { id: u64, bytes: Vec<u8>, argTypes: Vec<Type>, returnType: Type },
  /// todo desc
  CallPointer { pointer: usize, args: Vec<Value>, resultType: Type }
}

/// Response to the request with the execution result or error.
#[derive(Serialize, Deserialize)]
pub(super) enum FFIResponse
{
  /// Successful execution with the returned value.
  Ok(Value),
  /// Execution failed with the corresponding error.
  Err(FFIError)
}

/// Controls the zygote process and the communication channel with it.
pub(super) struct ZygoteHandle
{
  /// Child zygote process.
  process: Child,
  /// Socket for exchanging requests and responses with the process.
  pub(super) socket: UnixStream
}

impl Drop for ZygoteHandle
{
  /// Terminates the zygote process and removes the temporary socket.
  fn drop(&mut self) -> ()
  {
    let _ = self.process.kill();
  }
}

/// Global state of the active zygote with synchronized access.
pub(super) static ZygoteState: OnceLock<Mutex<ZygoteHandle>> = OnceLock::new();

// =================================================================================================

/// RAII handle for a separate zygote clone process.
pub struct ClonedZygote
{
  /// Process PID.
  pub pid: libc::pid_t,

  /// Socket for exchanging requests.
  pub socket: UnixStream
}

impl ClonedZygote
{
  /// Requests a clone from the main zygote and returns its RAII handle
  pub fn getMeClone() -> io::Result<Self>
  {
    let mutex: &Mutex<ZygoteHandle> = ZygoteState.get()
      .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Zygote not initialized"))?;
    let mut guard: MutexGuard<ZygoteHandle> = mutex.lock();

    // Sends a signal to create a clone
    writeMessage(&mut guard.socket, &[1u8])?;

    // Receives the clone PID
    let mut pidBytes: [u8; 4] = [0u8; 4];
    guard.socket.read_exact(&mut pidBytes)?;
    let pid: i32 = i32::from_le_bytes(pidBytes);

    // Read the socket descriptor directly from RAM
    let fd: RawFd = recvFd(&mut guard.socket)?;
    let socket: UnixStream = unsafe{ UnixStream::from_raw_fd(fd) };

    drop(guard);
    Ok(Self { pid, socket })
  }

  /// FFI call inside a specific clone.
  pub(super) fn call(&mut self, request: FFIRequest) -> Result<FFIResponse, String>
  {
    let bytes: Vec<u8> = encode(&request).map_err(|e| e.to_string())?;
    writeMessage(&mut self.socket, &bytes)
      .map_err(|e| format!("Zygote clone IPC failed: {}", e))?;

    let responseBytes: Vec<u8> = readMessage(&mut self.socket)
      .map_err(|e| format!("Zygote clone IPC failed: {}", e))?;
    let response: FFIResponse = decode(&responseBytes).map_err(|e| e.to_string())?;

    Ok(response)
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

thread_local!{
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
/// main() must call this as the first line if the first argument == ZygoteFlag;
///
/// The process is spawned through Command (fork+exec) — runtime was not warmed up,
/// there are no extra tasks, there is no metadata heap. The library is not loaded in advance.
pub (super) fn runAsZygote() -> !
{
  // Take the socket directly from STDIN
  let socket: UnixStream = unsafe{ UnixStream::from_raw_fd(libc::STDIN_FILENO) };
  zygoteLoop(socket);
}

/// Zygote initialization; call once, 
/// as the very first line of the normal main().
pub(super) fn initZygote() -> io::Result<()>
{
  let handle: ZygoteHandle = spawnZygote()?;
  ZygoteState.set(Mutex::new(handle))
    .map_err(|_| io::Error::new(io::ErrorKind::AlreadyExists, "Zygote already initialized"))?;
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
pub(super) fn spawnZygote() -> io::Result<ZygoteHandle>
{
  // Creates a socket pair directly in RAM without filesystem involvement
  let (runtimeSocket, zygoteSocket): (UnixStream, UnixStream) = UnixStream::pair()?;

  //
  let currentExe: PathBuf = env::current_exe()?;
  // todo Might fail if the path to the executable file 
  //  is too long or there are no permissions?
  let process: Child = Command::new(currentExe)
    .arg(ZygoteFlag)
    .stdin(Stdio::from(OwnedFd::from(zygoteSocket)))
    .spawn()?;

  Ok(ZygoteHandle{ process, socket: runtimeSocket })
}

/// Main zygote loop: an infinite command waiting loop.
/// Which FFI will be needed is unknown in advance.
///
/// The zygote is an empty runtime template; 
/// dlopen only works with the forked zygote.
fn zygoteLoop(mut socket: UnixStream) -> !
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

  //
  loop
  {
    // Get the socket path for the new clone
    if readMessage(&mut socket).is_err() {
      std::process::exit(0); // Parent died
    }

    // Create a paired socket in memory for the new clone
    let (runtimeSocket, cloneSocket): (UnixStream, UnixStream) = match UnixStream::pair() {
      Ok(pair) => pair,
      Err(_) => {
        let _ = writeMessage(&mut socket, &0i32.to_le_bytes());
        continue;
      }
    };

    match unsafe{ libc::fork() }
    {
      -1 => {
        let _ = writeMessage(&mut socket, &0i32.to_le_bytes());
      }
      0 => {
        // Zygote clone: close the parent side and enter the loop
        drop(runtimeSocket);
        cloneLoop(cloneSocket);
      }
      pid => {
        // Main zygote: send the PID and forward the socket descriptor to the Runtime
        drop(cloneSocket);
        if socket.write_all(&pid.to_le_bytes()).is_ok() {
          let _ = sendFd(&mut socket, runtimeSocket.as_raw_fd());
        }
      }
    }
    //
  }
}

/// Personal clone loop.
fn cloneLoop(mut socket: UnixStream) -> !
{
  let mut libraryCache: FxHashMap<String, Library> = FxHashMap::default();

  loop
  {
    let requestBytes: Vec<u8> = match readMessage(&mut socket)
    {
      Ok(bytes) => bytes,
      Err(_) =>
      // The variable was dropped — the socket was closed, the clone exited.
        std::process::exit(0)
    };

    let response: FFIResponse = handleRequest(&requestBytes, &mut libraryCache);
    let encodedResponse: Vec<u8> = match encode(&response) {
      Ok(bytes) => bytes,
      Err(_) => std::process::exit(1),
    };

    if writeMessage(&mut socket, &encodedResponse).is_err() {
      std::process::exit(0);
    }
  }
}

/// Handles an incoming request and performs an FFI operation using the library cache.
///
/// Returns the execution result or an error description.
fn handleRequest(requestBytes: &[u8], cache: &mut FxHashMap<String, Library>) -> FFIResponse
{
  match decode::<FFIRequest>(requestBytes)
  {
    Ok(request) => match executeFFI(request, cache)
    {
      Ok(v) => FFIResponse::Ok(v),
      Err(e) => FFIResponse::Err(e),
    },
    Err(e) => FFIResponse::Err(e),
  }
}

// =================================================================================================

/// Supervisor: blocks on the death of the current zygote (waitpid) and recreates it.
///
/// Separate thread — therefore spawnZygote() inside must go through Command, not fork().
fn supervisorLoop() -> ()
{
  loop
  {
    let pidToWait: u32 = {
      let mutex: &Mutex<ZygoteHandle> = match ZygoteState.get() { Some(m) => m, None => return };
      mutex.lock().process.id()
    };
    unsafe{ libc::waitpid(pidToWait as libc::pid_t, std::ptr::null_mut(), 0); }

    let mutex: &Mutex<ZygoteHandle> = ZygoteState.get().unwrap();
    let mut guard: MutexGuard<ZygoteHandle> = mutex.lock();
    if guard.process.id() == pidToWait // Not recreated in parallel yet through call()
    {
      match spawnZygote()
      {
        Ok(newHandle) => { *guard = newHandle; }
        Err(_) => { drop(guard); thread::sleep(std::time::Duration::from_millis(200)); }
      }
    }
    //
  }
}

// =================================================================================================

/// Writes a message to the socket with the data size prepended.
fn writeMessage(socket: &mut UnixStream, data: &[u8]) -> io::Result<()>
{
  socket.write_all(&(data.len() as u32).to_le_bytes())?;
  socket.write_all(data)
}

/// Reads a message from the socket using the length specified in the header.
fn readMessage(socket: &mut UnixStream) -> io::Result<Vec<u8>>
{
  let mut lengthBuffer: [u8; 4] = [0u8; 4];
  socket.read_exact(&mut lengthBuffer)?;
  let mut buffer: Vec<u8> = vec![0u8; u32::from_le_bytes(lengthBuffer) as usize];
  socket.read_exact(&mut buffer)?;
  Ok(buffer)
}

/// Serializes a value into a byte representation.
pub(super) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, FFIError>
{
  let config: Configuration = bincode::config::standard();
  bincode::serde::encode_to_vec(value, config)
    .map_err(|e| FFIError::EncodeFailed(format!("Encode failed: {}", e)))
}

/// Deserializes a byte representation back into a value.
pub(super) fn decode<T: for<'a> Deserialize<'a>>(bytes: &[u8]) -> Result<T, FFIError>
{
  let config: Configuration = bincode::config::standard();
  bincode::serde::decode_from_slice(bytes, config)
    .map(|(decoded, _)| decoded)
    .map_err(|e| FFIError::DecodeFailed(format!("Decode failed: {}", e)))
}

// =================================================================================================

/// Sends the socket descriptor to 
/// another process through an anonymous channel.
fn sendFd(socket: &mut UnixStream, fd: RawFd) -> io::Result<()>
{
  // According to the POSIX standard, 
  // at least 1 byte of actual data is required to send cmsg.
  let mut msgHeader: libc::msghdr = unsafe { std::mem::zeroed() };
  let mut dummyByte: [u8; 1] = [0u8; 1];

  let mut ioVector: libc::iovec = libc::iovec {
    iov_base: dummyByte.as_mut_ptr() as *mut _,
    iov_len: 1,
  };

  // Allocate memory for the ancillary message 
  // and pack the FD into the SCM_RIGHTS structure.
  let cmsgSpace: u32 = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) };
  let mut cmsgBuffer: Vec<u8> = vec![0u8; cmsgSpace as usize];

  msgHeader.msg_iov = &mut ioVector;
  msgHeader.msg_iovlen = 1;
  msgHeader.msg_control = cmsgBuffer.as_mut_ptr() as *mut _;
  msgHeader.msg_controllen = cmsgBuffer.len() as _;

  unsafe {
    let cmsg: *mut libc::cmsghdr = libc::CMSG_FIRSTHDR(&msgHeader);
    (*cmsg).cmsg_level = libc::SOL_SOCKET;
    (*cmsg).cmsg_type = libc::SCM_RIGHTS;
    (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;

    let fdPtr: *mut RawFd = libc::CMSG_DATA(cmsg) as *mut RawFd;
    fdPtr.write_unaligned(fd);
  }

  // Send the control packet through the kernel system call
  let result: libc::ssize_t = unsafe { libc::sendmsg(socket.as_raw_fd(), &msgHeader, 0) };
  if result < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Receives the socket descriptor directly 
/// from the memory of another process.
fn recvFd(socket: &mut UnixStream) -> io::Result<RawFd>
{
  // Prepare buffers to receive the dummy byte and the ancillary header
  let mut msgHeader: libc::msghdr = unsafe { std::mem::zeroed() };
  let mut dummyByte: [u8; 1] = [0u8; 1];

  let mut ioVector: libc::iovec = libc::iovec {
    iov_base: dummyByte.as_mut_ptr() as *mut _,
    iov_len: 1,
  };

  let cmsgSpace: u32 = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) };
  let mut cmsgBuffer: Vec<u8> = vec![0u8; cmsgSpace as usize];

  msgHeader.msg_iov = &mut ioVector;
  msgHeader.msg_iovlen = 1;
  msgHeader.msg_control = cmsgBuffer.as_mut_ptr() as *mut _;
  msgHeader.msg_controllen = cmsgBuffer.len() as _;

  // Read the message from the socket
  let result: libc::ssize_t = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut msgHeader as *mut _, 0) };
  if result <= 0 { return Err(io::Error::last_os_error()); }

  // Check for access permissions and extract the received descriptor
  unsafe {
    let cmsg: *mut libc::cmsghdr = libc::CMSG_FIRSTHDR(&msgHeader);
    if cmsg.is_null() || (*cmsg).cmsg_type != libc::SCM_RIGHTS {
      return Err(io::Error::new(io::ErrorKind::InvalidData, "No FD received"));
    }

    let fdPtr: *const RawFd = libc::CMSG_DATA(cmsg) as *const RawFd;
    Ok(fdPtr.read_unaligned())
  }
}

// =================================================================================================