use chillffi::ffi::value::Value;
use chillffi::ffi::errors::FFIError;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::callv;
use chillffi::call;
use chillffi::ffi;
// =================================================================================================

/// Create IPC pipe, write data, and read back via libc.
fn main() -> ()
{
  let received: Vec<u8> = ffi!(|scope| {
    let libc: Library = Library::load("libc.so.6")?;

    // Allocate memory for pipefd array (2 ints)
    let fdsMem: AllocatedMemory = scope.alloc(8)?; // int pipefd[2]
    
    // Call pipe() to obtain read and write file descriptors
    let result: i32 = call!(libc, "pipe", fdsMem.asPointer())?;
    if result != 0 {
      return Err(FFIError::Other("pipe() failed".into()));
    }

    // Read file descriptors from memory block
    let Value::RawString(fdsBytes) = fdsMem.read()? else {
      return Err(FFIError::Other("expected bytes".into()))
    };
    
    // Parse read and write file descriptors
    let readFd: i32 = i32::from_ne_bytes(fdsBytes[0..4].try_into().unwrap());
    let writeFd: i32 = i32::from_ne_bytes(fdsBytes[4..8].try_into().unwrap());

    // Write payload to write end of pipe
    callv!(libc, "write", writeFd, Value::RawString(b"hi".to_vec()), 2 as usize)?;

    // Allocate memory buffer for reading
    let bufMem: AllocatedMemory = scope.alloc(2)?;
    
    // Read payload from read end of pipe into buffer
    callv!(libc, "read", readFd, bufMem.asPointer(), 2 as usize)?;

    // Read buffer contents from memory block
    let Value::RawString(readBytes) = bufMem.read()? else {
      return Err(FFIError::Other("expected bytes".into()))
    };

    // Close file descriptors
    callv!(libc, "close", readFd)?;
    callv!(libc, "close", writeFd)?;

    Ok(readBytes)
  }).expect("pipe roundtrip failed");

  //
  println!("received: {:?}", String::from_utf8_lossy(&received));

  assert_eq!(received, b"hi".to_vec());
  println!("ok: pipe roundtrip");
}

// =================================================================================================