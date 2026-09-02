use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::errors::FFIError;
use chillffi::ffi;
// =================================================================================================

/// Create IPC pipe, write data, and read back via libc.
fn main() -> ()
{
  let received: Vec<u8> = ffi!(|scope| {
    let libc: Library = scope.load("libc.so.6")?;

    // Allocate memory for pipefd array (2 ints)
    let fdsMem: AllocatedMemory = scope.alloc(8)?; // int pipefd[2]
    
    // Call pipe() to obtain read and write file descriptors
    let result: i32 = libc.call("pipe").arg(fdsMem.asPointer()).result()?;
    if result != 0 {
      return Err(FFIError::Other("pipe() failed".into()));
    }

    // Read file descriptors from memory block
    let fdsBytes: Vec<u8> = fdsMem.read()?;
    
    // Parse read and write file descriptors
    let readFd: i32 = i32::from_ne_bytes(fdsBytes[0..4].try_into().unwrap());
    let writeFd: i32 = i32::from_ne_bytes(fdsBytes[4..8].try_into().unwrap());

    // Write payload to write end of pipe
    libc.call("write")
      .arg(writeFd)
      .arg(b"hi".to_vec())
      .arg::<usize>(2)
      .void()?;

    // Allocate memory buffer for reading
    let bufMem: AllocatedMemory = scope.alloc(2)?;
    
    // Read payload from read end of pipe into buffer
    libc.call("read")
      .arg(readFd)
      .arg(bufMem.asPointer())
      .arg::<usize>(2)
      .void()?;

    // Read buffer contents from memory block
    let readBytes: Vec<u8> = bufMem.read()?;

    // Close file descriptors
    libc.call("close").arg(readFd).void()?;
    libc.call("close").arg(writeFd).void()?;

    Ok(readBytes)
  }).expect("pipe roundtrip failed");

  //
  println!("received: {:?}", String::from_utf8_lossy(&received));

  assert_eq!(received, b"hi".to_vec());
  println!("ok: pipe roundtrip");
}

// =================================================================================================