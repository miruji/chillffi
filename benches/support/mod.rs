//! Shared by `warm.rs` and `cold.rs`. Each of them uses only a part of it.
#![allow(dead_code)]

use pyo3::prelude::*;
use pyo3::types::PyModule;
use std::ffi::CString;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

#[cfg(all(unix, not(target_os = "macos")))]
pub const libcPath: &str = "libc.so.6";
#[cfg(target_os = "macos")]
pub const libcPath: &str = "libSystem.dylib";

pub const payloadSize: usize = 1 << 16;

pub type CFunction = unsafe extern "C" fn(i32) -> i32;

unsafe extern "C" { pub fn toupper(character: i32) -> i32; }

pub fn openLibrary() -> libloading::Library
{
  unsafe{ libloading::Library::new(libcPath) }.unwrap()
}

pub fn loadToupper(lib: &libloading::Library) -> CFunction
{
  let symbol: libloading::Symbol<CFunction> = unsafe{ lib.get(b"toupper\0") }.unwrap();
  *symbol
}

/// Divan does not warm up. Every row runs its operation this many times before it is measured.
pub fn warmUp(times: usize, mut operation: impl FnMut() -> ()) -> ()
{
  for _ in 0..times { operation(); }
}

/// Runs `body` with `python.py` loaded into the embedded interpreter. Loading is not timed by the callers.
pub fn python<R>(body: impl FnOnce(&Bound<'_, PyModule>) -> PyResult<R>) -> R
{
  Python::attach(|py| {
    let code: CString = CString::new(include_str!("python.py")).unwrap();
    let module: Bound<'_, PyModule> = PyModule::from_code(py, code.as_c_str(), c"python.py", c"python").unwrap();
    module.setattr("libcPath", libcPath).unwrap();
    body(&module).unwrap()
  })
}

/// Persistent forked peer, torn down on drop. `size` bytes go in, `size` bytes come back.
/// With `callFunction` the peer treats the first 4 bytes as i32 and answers `toupper` of it.
pub struct Peer
{
  stream: Option<UnixStream>,
  pid: i32
}

impl Peer
{
  pub fn spawn(size: usize, callFunction: bool) -> Self
  {
    let lib: libloading::Library = openLibrary();
    let function: CFunction = loadToupper(&lib);
    let (left, mut right): (UnixStream, UnixStream) = UnixStream::pair().unwrap();
    let mut reply: Vec<u8> = vec![0; size];

    let pid: i32 = unsafe{ libc::fork() };
    if pid == 0
    {
      drop(left);
      while right.read_exact(&mut reply).is_ok()
      {
        if callFunction
        {
          let value: i32 = i32::from_ne_bytes(reply[0..4].try_into().unwrap());
          reply[0..4].copy_from_slice(&unsafe{ function(value) }.to_ne_bytes());
        }
        if right.write_all(&reply).is_err() { break; }
      }
      unsafe{ libc::_exit(0) };
    }
    Self{ stream: Some(left), pid }
  }

  pub fn roundTrip(&mut self, request: &[u8], reply: &mut [u8]) -> ()
  {
    let stream: &mut UnixStream = self.stream.as_mut().unwrap();
    stream.write_all(request).unwrap();
    stream.read_exact(reply).unwrap();
  }
}

impl Drop for Peer
{
  fn drop(&mut self) -> ()
  {
    drop(self.stream.take());
    unsafe{ libc::waitpid(self.pid, std::ptr::null_mut(), 0) };
  }
}

/// Persistent forked peer with the same shape as chillffi's memory API: two round trips per pair of operations.
/// write = one request carrying the payload (first byte is the opcode 1) and a 1-byte ack;
/// read  = one request (opcode 2) and one reply carrying the payload.
pub struct BufferPeer
{
  stream: Option<UnixStream>,
  pid: i32
}

impl BufferPeer
{
  pub fn spawn(size: usize) -> Self
  {
    let (left, mut right): (UnixStream, UnixStream) = UnixStream::pair().unwrap();
    let mut buffer: Vec<u8> = vec![0; size];

    let pid: i32 = unsafe{ libc::fork() };
    if pid == 0
    {
      drop(left);
      let mut opcode: [u8; 1] = [0];
      while right.read_exact(&mut opcode).is_ok()
      {
        let done: bool = match opcode[0]
        {
          1 => right.read_exact(&mut buffer).is_ok() && right.write_all(&[1]).is_ok(),
          2 => right.write_all(&buffer).is_ok(),
          _ => false
        };
        if !done { break; }
      }
      unsafe{ libc::_exit(0) };
    }
    Self{ stream: Some(left), pid }
  }

  /// `request` is `[1]` followed by the payload.
  pub fn write(&mut self, request: &[u8]) -> ()
  {
    let stream: &mut UnixStream = self.stream.as_mut().unwrap();
    stream.write_all(request).unwrap();
    let mut ack: [u8; 1] = [0];
    stream.read_exact(&mut ack).unwrap();
  }

  pub fn read(&mut self, reply: &mut [u8]) -> ()
  {
    let stream: &mut UnixStream = self.stream.as_mut().unwrap();
    stream.write_all(&[2]).unwrap();
    stream.read_exact(reply).unwrap();
  }
}

impl Drop for BufferPeer
{
  fn drop(&mut self) -> ()
  {
    drop(self.stream.take());
    unsafe{ libc::waitpid(self.pid, std::ptr::null_mut(), 0) };
  }
}
