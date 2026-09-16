#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::LibcPath;
// =================================================================================================
use chillffi::ffi;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::errors::FFIError;
// =================================================================================================

/// Raw `AllocatedMemory`: `alloc` / `allocAligned` and manual byte read/write.
fn main() -> ()
{
  allocAligned();
  pipeRoundtrip();
}

// =================================================================================================

/// `allocAligned` when you need a specific alignment (posix_memalign).
fn allocAligned() -> ()
{
  let addr: usize = ffi!(|scope| {
    let mem: AllocatedMemory = scope.allocAligned(64, 16)?; // 64 bytes, 16-byte aligned
    Ok(usize::from(mem.asPointer()))
  }).expect("allocAligned failed");

  assert_eq!(addr % 16, 0, "posix_memalign was required to return an aligned address");
  println!("ok: allocAligned(64, 16) -> 0x{addr:X}, % 16 == 0");
}

/// `pipe()` writes two fds into an out-parameter — typical use of AllocatedMemory.
fn pipeRoundtrip() -> ()
{
  let received: Vec<u8> = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;

    // int pipefd[2]; — the out-parameter pipe() fills in.
    let fdsMem: AllocatedMemory = scope.alloc(8)?;
    let result: i32 = libc.call("pipe").arg(fdsMem.asPointer()).result()?;
    if result != 0 {
      return Err(FFIError::Other("pipe() failed".into()));
    }

    let fdsBytes: Vec<u8> = fdsMem.read()?;
    let readFd: i32 = i32::from_ne_bytes(fdsBytes[0..4].try_into().unwrap());
    let writeFd: i32 = i32::from_ne_bytes(fdsBytes[4..8].try_into().unwrap());

    libc.call("write").arg(writeFd).arg(b"hi".to_vec()).arg::<usize>(2).void()?;

    let bufMem: AllocatedMemory = scope.alloc(2)?;
    libc.call("read").arg(readFd).arg(bufMem.asPointer()).arg::<usize>(2).void()?;
    let readBytes: Vec<u8> = bufMem.read()?;

    libc.call("close").arg(readFd).void()?;
    libc.call("close").arg(writeFd).void()?;

    Ok(readBytes)
  }).expect("pipe roundtrip failed");

  assert_eq!(received, b"hi".to_vec());
  println!("ok: pipe roundtrip -> {:?}", String::from_utf8_lossy(&received));
}

// =================================================================================================
