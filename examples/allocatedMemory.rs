use chillffi::ffi::value::Value;
use chillffi::ffi::errors::FFIError;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::call;
use chillffi::ffi;
// =================================================================================================

/// Call stat() via libc and read file size from struct out-parameter.
fn main() -> ()
{
  let size: i64 = ffi!(|scope| {
    let libc: Library = Library::load("libc.so.6")?;

    // struct stat — 144 bytes on x86_64 Linux (glibc)
    // Allocate memory for struct stat out-parameter
    let statMem: AllocatedMemory = scope.alloc(144)?;

    // Call stat() with path and allocated buffer
    let result: i32 = call!(
      libc, "stat",
      Value::CString(b"/etc/hostname".to_vec()),
      statMem.asPointer()
    )?;
    if result != 0 {
      return Err(FFIError::Other("stat() returned non-zero".into()));
    }

    // Read populated memory block back to parent process
    let Value::RawString(bytes) = statMem.read()? else {
      return Err(FFIError::Other("expected bytes".into()))
    };

    // st_size — offset 48
    // Parse st_size from raw bytes
    Ok(i64::from_ne_bytes(bytes[48..56].try_into().unwrap()))
  }).expect("stat() failed");

  //
  println!("file size = {} bytes", size);

  let expected: u64 = std::fs::metadata("/etc/hostname").expect("metadata").len();
  assert_eq!(size as u64, expected);
  println!("ok: stat via libc");
}

// =================================================================================================