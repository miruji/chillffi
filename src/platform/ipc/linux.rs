//! Linux backend for [`super::Transport`].
//!
//! Pure `libc`, no `ipc-channel`.
//!
//! - **Control plane** (Runtime ↔ Main Zygote): one `UnixStream` pair created
//!   in the Runtime; the Zygote-side end is handed to the freshly spawned
//!   Zygote as `stdin` (matching the original `Command::new(...).stdin(...)`
//!   pattern from `0.3.0` — `exec()` resets a multithreaded runtime's locked
//!   mutexes, so we never `fork()` straight from Runtime here).
//!
//! - **Data plane** (Runtime ↔ Clone): a fresh `UnixStream::pair()` per clone,
//!   created inside Main Zygote just before `libc::fork()`. The clone-side end
//!   stays in the child through the `fork` (no OOB handoff needed); the
//!   Runtime-side end is forwarded to the Runtime over the control channel
//!   via `SCM_RIGHTS` (`sendFd` / `recvFd`).
//!
//! - **Frame format**: `[u32 LE length][bincode payload]` for the control
//!   plane; for the data plane the same framing is used (one message = one
//!   serialized `FFIRequest` / `FFIResponse`).
// =================================================================================================
use super::Transport as TransportTrait;
use super::{
  CloneSide as CloneSideTrait, FFIRequest, FFIResponse,
  RuntimeSide as RuntimeSideTrait, ZygoteFlag, ZygoteHandleBase
};
use crate::ffi::errors::FFIError;
use crate::worker::executeFFI;
use crate::worker::{takeLastErrno, takeLastOsError};
use bincode::config::Configuration;
use fxhash::FxHashMap;
use libloading::Library;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::env;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
// =================================================================================================

/// Backend tag used in diagnostics.
const BackendName: &str = "linux-libc";

// =================================================================================================

/// Serializes a value into a byte representation.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, FFIError>
{
  let config: Configuration = bincode::config::standard();
  bincode::serde::encode_to_vec(value, config)
    .map_err(|e| FFIError::EncodeFailed(format!("Encode failed: {}", e)))
}

/// Deserializes a byte representation back into a value.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, FFIError>
{
  let config: Configuration = bincode::config::standard();
  bincode::serde::decode_from_slice(bytes, config)
    .map(|(decoded, _)| decoded)
    .map_err(|e| FFIError::DecodeFailed(format!("Decode failed: {}", e)))
}

// =================================================================================================

/// Linux Transport: `libc`-based IPC, no `ipc-channel`.
pub struct Transport;

/// Runtime-side handle to the Main Zygote: child `process` plus the
/// control-plane socket (Runtime end of the `UnixStream` pair).
pub struct ZygoteHandle
{
  /// Common handle (process handle + Drop).
  pub base: ZygoteHandleBase,
  
  /// Runtime end of the control-plane socket pair.
  pub controlSocket: UnixStream
}

impl Drop for ZygoteHandle
{
  fn drop(&mut self) -> ()
  {
    // `base` already kills the process on drop; nothing extra to do.
    // todo тут что-то было раньше? вроде было. или нужно?
  }
}

/// Runtime-side data endpoint.
pub struct RuntimeSide
{
  /// Data-plane socket — Runtime end.
  pub socket: UnixStream
}

/// Clone-side data endpoint.
pub struct CloneSide
{
  /// Data-plane socket — Clone end.
  #[allow(dead_code)]
  pub socket: UnixStream
}

/// Bootstrap carried through the control channel from a freshly cloned
/// process back to the Runtime. The Runtime then rebuilds its data endpoint
/// via [`Transport::runtimeConnect`].
#[derive(Serialize, Deserialize)]
pub struct Bootstrap
{
  /// PID of the clone at the moment the control channel reported it.
  pub pid: u32,
  
  /// Raw FD for the data-plane socket, transferred via `SCM_RIGHTS`.
  pub fd: RawFd
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

  /// Spawns the Main Zygote.
  ///
  /// Same rationale as `0.3.0`: a direct `fork()` from the warmed-up,
  /// multithreaded Runtime would inherit locked mutexes (the supervisor
  /// thread holds some). `Command::new(current_exe).stdin(zygoteSocket)`
  /// forks **and** execs — `exec()` wipes the inherited lock state and the
  /// Zygote is born clean.
  fn spawnZygote() -> io::Result<Self::ZygoteHandle>
  {
    // Pair of control-plane sockets — the Zygote end is handed off as stdin.
    let (controlRuntime, controlZygote): (UnixStream, UnixStream) =
      UnixStream::pair()?;

    //
    let currentExe: PathBuf = env::current_exe()?;
    // todo Might fail if the path to the executable file
    //  is too long or there are no permissions?
    let process: Child = Command::new(currentExe)
      .arg(ZygoteFlag)
      .stdin(Stdio::from(OwnedFd::from(controlZygote)))
      .stdout(Stdio::inherit())
      .stderr(Stdio::inherit())
      .spawn()?;

    Ok(ZygoteHandle {
      base: ZygoteHandleBase { process },
      controlSocket: controlRuntime
    })
  }

  /// Runtime asks Main Zygote to fork a clone and returns its bootstrap.
  ///
  /// Wire format on the control plane:
  ///   1. Request  : `[u32 LE length == 1][0x01]`
  ///   2. Reply    : `[i32 LE pid]` then 1 dummy byte carrying one `SCM_RIGHTS` FD.
  fn sendSpawnClone(handle: &Self::ZygoteHandle) -> io::Result<Self::Bootstrap>
  {
    let controlFd: RawFd = handle.controlSocket.as_raw_fd();

    // "spawn clone" — a single-byte message so we don't allocate.
    writeMessage(controlFd, &[1u8])?;

    // PID.
    let mut pidBuf: [u8; 4] = [0u8; 4];
    recvExact(controlFd, &mut pidBuf)?;
    let pid: u32 = u32::from_le_bytes(pidBuf);

    // Data-plane socket FD over `SCM_RIGHTS`.
    let fd: RawFd = recvFd(controlFd)?;

    Ok(Bootstrap { pid, fd })
  }

  /// todo desc
  fn bootstrapPid(bootstrap: &Self::Bootstrap) -> u32
  {
    bootstrap.pid
  }

  /// Enters the Main Zygote command loop. Called inside the freshly spawned
  /// Zygote (before any clone exists). The control plane arrives as `stdin`
  /// (see [`Transport::spawnZygote`]). Never returns.
  ///
  /// On Linux `cloneEnter` is never invoked — `cloneLoopInner` runs in the
  /// child directly, because `fork()` hands the clone its end of the
  /// data-plane pair as a normal Rust variable. The control plane is
  /// parent-only.
  fn zygoteControlLoop(_flag: Option<String>) -> !
  {
    let controlSocket: UnixStream =
      unsafe{ UnixStream::from_raw_fd(libc::STDIN_FILENO) };

    // Important: Ignoring SIGCHLD is needed only in the main Zygote.
    // This makes the OS kernel automatically clean up its clones on
    // termination (without zombies). It must not be written in the main
    // Runtime: there, `waitpid` in `supervisorLoop` tracks the Zygote
    // process itself, and with SIG_IGN it would fail with ECHILD and
    // enter guaranteed CPU load.
    unsafe{ libc::signal(libc::SIGCHLD, libc::SIG_IGN); }

    zygoteLoop(controlSocket);
  }

  /// Not used on Linux — see [`Transport::zygoteControlLoop`]. Dispatched
  /// through the trait, so Clippy sees it as "never used" — silenced here.
  #[allow(dead_code)]
  fn cloneEnter(_flag: Option<String>) -> io::Result<(Self::CloneSide, Self::Bootstrap)>
  {
    unreachable!("linux::cloneEnter is never called; the child runs cloneLoopInner directly")
  }

  /// Wraps the FD received via `SCM_RIGHTS` back into a `UnixStream`.
  fn runtimeConnect(bootstrap: Self::Bootstrap) -> io::Result<Self::RuntimeSide>
  {
    let socket: UnixStream = unsafe{ UnixStream::from_raw_fd(bootstrap.fd) };
    Ok(RuntimeSide { socket })
  }
}

// =================================================================================================

impl RuntimeSideTrait for RuntimeSide
{
  /// todo desc
  fn send(&self, request: &FFIRequest) -> Result<(), String>
  {
    let bytes: Vec<u8> = encode(request).map_err(|e| e.to_string())?;
    writeMessage(self.socket.as_raw_fd(), &bytes)
      .map_err(|e| format!("Zygote clone IPC failed while sending request: {e}"))
  }

  /// todo desc
  fn recv(&self) -> Result<FFIResponse, String>
  {
    let bytes: Vec<u8> = readMessage(self.socket.as_raw_fd())
      .map_err(|e| format!("Zygote clone IPC failed while reading response: {e}"))?;
    decode(&bytes).map_err(|e| e.to_string())
  }
}

impl CloneSide
{
  /// Builds a `CloneSide` from an already-existing socket. Used by
  /// `zygoteLoop` after `fork()` — bypasses
  /// [`super::Transport::cloneEnter`] (which is unreachable on Linux).
  #[allow(dead_code)]
  pub const fn fromSocket(socket: UnixStream) -> Self
  {
    Self { socket }
  }
}

impl CloneSideTrait for CloneSide
{
  /// Runs the per-clone request/response loop until the Runtime closes the
  /// socket or a fatal error occurs. Never returns. Dispatched through the
  /// trait, so Clippy sees it as "never used" — silenced here.
  #[allow(dead_code)]
  fn run(self, cache: &mut FxHashMap<String, Library>) -> !
  {
    cloneLoop(self.socket, cache)
  }
}

// =================================================================================================

/// Main zygote loop: an infinite command waiting loop.
/// Which FFI will be needed is unknown in advance.
///
/// The zygote is an empty runtime template;
/// `dlopen` only works with the forked zygote.
fn zygoteLoop(controlSocket: UnixStream) -> !
{
  let controlFd: RawFd = controlSocket.as_raw_fd();
  loop
  {
    // Wait for "spawn clone" from the Runtime.
    if readMessage(controlFd).is_err() {
      // Parent (Runtime) closed the control plane — exit cleanly.
      std::process::exit(0);
    }

    // Create a paired socket in memory for the new clone.
    let (dataForRuntime, dataForClone): (UnixStream, UnixStream) =
      match UnixStream::pair() {
        Ok(pair) => pair,
        Err(_) => {
          // todo Could be reported back to Runtime through control plane;
          //  for parity with 0.3.0 we just swallow it (Runtime will time out
          //  on `recvExact` and surface its own error).
          continue;
        }
      };

    match unsafe{ libc::fork() }
    {
      -1 => 
      { // Fork failed — drop both ends, wait for the next request.
        drop(dataForRuntime);
        drop(dataForClone);
      }
      0 => 
      { // Zygote clone: close the Runtime end of data plane, close our
        // (inherited) control plane, and enter the loop. The control plane
        // is parent-only on Linux; clones don't speak it.
        drop(dataForRuntime);
        drop(controlSocket);

        let cache: &mut FxHashMap<String, Library> =
          Box::leak(Box::new(FxHashMap::default()));
        cloneLoop(dataForClone, cache);
      }
      pid => 
      { // Main zygote: close the clone end of data plane, send PID and the
        // Runtime end's FD back over the control plane.
        drop(dataForClone);
        let pidBytes: [u8; 4] = (pid as u32).to_le_bytes();
        if sendAll(controlFd, &pidBytes).is_ok() {
          let _ = sendFd(controlFd, dataForRuntime.as_raw_fd());
        }
        drop(dataForRuntime);
      }
    }
  }
}

/// Per-clone request/response loop. Never returns.
///
/// Any I/O error means the Runtime closed the pipe (or the clone died) —
/// the clone `std::process::exit(0)`s and the kernel reaps it (because
/// `SIGCHLD` is ignored in Main Zygote).
fn cloneLoop(socket: UnixStream, cache: &mut FxHashMap<String, Library>) -> !
{
  let fd: RawFd = socket.as_raw_fd();
  loop
  {
    let requestBytes: Vec<u8> = match readMessage(fd)
    {
      Ok(bytes) => bytes,
      Err(_) => std::process::exit(0)
    };

    let response: FFIResponse = handleRequest(&requestBytes, cache);
    let encoded: Vec<u8> = match encode(&response)
    {
      Ok(bytes) => bytes,
      Err(_) => std::process::exit(1)
    };

    if writeMessage(fd, &encoded).is_err() {
      std::process::exit(0);
    }
  }
}

/// Handles an incoming request and performs an FFI operation using the library cache.
fn handleRequest(
  requestBytes: &[u8],
  cache: &mut FxHashMap<String, Library>
) -> FFIResponse
{
  match decode::<FFIRequest>(requestBytes)
  {
    Ok(request) => match executeFFI(request, cache)
    {
      // `takeLastErrno` reads whatever `invokeFFI` stashed right after
      // `cif.call()` (or `None`, for requests that never call — Alloc,
      // Free, ReadMemory, ... and for calls that didn't ask for it) —
      // and clears it for the next request.
      Ok(v) => FFIResponse::Ok(v, takeLastErrno(), takeLastOsError()),
      Err(e) => FFIResponse::Err(e)
    },
    Err(e) => FFIResponse::Err(e)
  }
}

// =================================================================================================

/// Writes a message to `fd` with the data size prepended. Uses raw `libc::send`
/// so the caller can pass an immutable `RawFd` (no `&mut` borrow conflicts
/// with the trait's `&self` methods).
fn writeMessage(fd: RawFd, data: &[u8]) -> io::Result<()>
{
  let lenBytes: [u8; 4] = (data.len() as u32).to_le_bytes();
  sendAll(fd, &lenBytes)?;
  sendAll(fd, data)
}

/// Reads a message from `fd` using the length specified in the header.
fn readMessage(fd: RawFd) -> io::Result<Vec<u8>>
{
  let mut lengthBuffer: [u8; 4] = [0u8; 4];
  recvExact(fd, &mut lengthBuffer)?;
  let mut buffer: Vec<u8> =
    vec![0u8; u32::from_le_bytes(lengthBuffer) as usize];
  recvExact(fd, &mut buffer)?;
  Ok(buffer)
}

/// Loops `libc::send` until the whole buffer has been written or an error
/// occurs.
fn sendAll(fd: RawFd, mut buf: &[u8]) -> io::Result<()>
{
  while !buf.is_empty()
  {
    let n: libc::ssize_t = unsafe{
      libc::send(
        fd,
        buf.as_ptr() as *const _,
        buf.len() as libc::size_t,
        0
      )
    };
    if n < 0
    {
      let err: io::Error = io::Error::last_os_error();
      if err.kind() == io::ErrorKind::Interrupted
      {
        continue;
      }
      return Err(err);
    }
    if n == 0
    {
      return Err(io::Error::new(
        io::ErrorKind::WriteZero,
        "send returned 0"
      ));
    }
    buf = &buf[n as usize..];
  }
  Ok(())
}

/// Loops `libc::recv` until `dst` is filled.
fn recvExact(fd: RawFd, dst: &mut [u8]) -> io::Result<()>
{
  let mut filled: usize = 0;
  while filled < dst.len()
  {
    let n: libc::ssize_t = unsafe{
      libc::recv(
        fd,
        dst[filled..].as_mut_ptr() as *mut _,
        (dst.len() - filled) as libc::size_t,
        0
      )
    };
    if n < 0
    {
      let err: io::Error = io::Error::last_os_error();
      if err.kind() == io::ErrorKind::Interrupted
      {
        continue;
      }
      return Err(err);
    }
    if n == 0
    {
      return Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "recv returned 0 before filling buffer"
      ));
    }
    filled += n as usize;
  }
  Ok(())
}

// =================================================================================================

/// Sends the socket descriptor to another process through an anonymous channel.
fn sendFd(socketFd: RawFd, fd: RawFd) -> io::Result<()>
{
  // According to the POSIX standard, at least 1 byte of actual data is
  // required to send cmsg.
  let mut msgHeader: libc::msghdr = unsafe{ MaybeUninit::zeroed().assume_init() };
  let mut dummyByte: [u8; 1] = [0u8; 1];

  let mut ioVector: libc::iovec = libc::iovec {
    iov_base: dummyByte.as_mut_ptr() as *mut _,
    iov_len: 1
  };

  // Allocate memory for the ancillary message and pack the FD into the
  // SCM_RIGHTS structure.
  let cmsgSpace: u32 = unsafe{ libc::CMSG_SPACE(size_of::<RawFd>() as u32) };
  let mut cmsgBuffer: Vec<u8> = vec![0u8; cmsgSpace as usize];

  msgHeader.msg_iov = &mut ioVector;
  msgHeader.msg_iovlen = 1;
  msgHeader.msg_control = cmsgBuffer.as_mut_ptr() as *mut _;
  msgHeader.msg_controllen = cmsgBuffer.len() as _;

  unsafe{
    let cmsg: *mut libc::cmsghdr = libc::CMSG_FIRSTHDR(&msgHeader);
    (*cmsg).cmsg_level = libc::SOL_SOCKET;
    (*cmsg).cmsg_type = libc::SCM_RIGHTS;
    (*cmsg).cmsg_len =
      libc::CMSG_LEN(size_of::<RawFd>() as u32) as _;

    let fdPtr: *mut RawFd = libc::CMSG_DATA(cmsg) as *mut RawFd;
    fdPtr.write_unaligned(fd);
  }

  // Send the control packet through the kernel system call.
  let result: libc::ssize_t = unsafe{ libc::sendmsg(socketFd, &msgHeader, 0) };
  if result < 0
  {
    Err(io::Error::last_os_error())
  } else {
    Ok(())
  }
}

/// Receives the socket descriptor directly from the memory of another process.
fn recvFd(socketFd: RawFd) -> io::Result<RawFd>
{
  // Prepare buffers to receive the dummy byte and the ancillary header.
  let mut msgHeader: libc::msghdr = unsafe{ MaybeUninit::zeroed().assume_init() };
  let mut dummyByte: [u8; 1] = [0u8; 1];

  let mut ioVector: libc::iovec = libc::iovec {
    iov_base: dummyByte.as_mut_ptr() as *mut _,
    iov_len: 1
  };

  let cmsgSpace: u32 = unsafe{ libc::CMSG_SPACE(size_of::<RawFd>() as u32) };
  let mut cmsgBuffer: Vec<u8> = vec![0u8; cmsgSpace as usize];

  msgHeader.msg_iov = &mut ioVector;
  msgHeader.msg_iovlen = 1;
  msgHeader.msg_control = cmsgBuffer.as_mut_ptr() as *mut _;
  msgHeader.msg_controllen = cmsgBuffer.len() as _;

  // Read the message from the socket.
  let result: libc::ssize_t = unsafe{ libc::recvmsg(socketFd, &mut msgHeader as *mut _, 0) };
  if result <= 0
  {
    return Err(io::Error::last_os_error());
  }

  // Check for access permissions and extract the received descriptor.
  unsafe{
    let cmsg: *mut libc::cmsghdr = libc::CMSG_FIRSTHDR(&msgHeader);
    if cmsg.is_null() || (*cmsg).cmsg_type != libc::SCM_RIGHTS
    {
      return Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "No FD received"
      ));
    }

    let fdPtr: *const RawFd = libc::CMSG_DATA(cmsg) as *const RawFd;
    Ok(fdPtr.read_unaligned())
  }
}

// =================================================================================================
