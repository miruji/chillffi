use chillffi::ffi::library::Library;
use chillffi::ffi::scope::{FFIScope, Scope};
use chillffi::ffi::value::Value;
use chillffi::ffi::errors::FFIError;
// =================================================================================================

/// Get file size via libc's stat using the non-macro FFIScope entry point.
///
/// This mirrors the `ffi!` macro behavior but provides manual control over the
/// scope's lifetime, which a code generator would use when block boundaries 
/// are not known at compile time.
fn main() -> ()
{
  let size: i64 = (|| -> Result<i64, FFIError> {
    // Open a manual FFI context.
    let ffiScope: FFIScope = FFIScope::enter()?;
    let scope: Scope<'_> = ffiScope.scope();

    let libc: Library = scope.load("libc.so.6")?;

    // Allocate memory for the out-parameter.
    // struct stat — 144 bytes on x86_64 Linux (glibc)
    let statMem = scope.alloc(144)?;

    // Invoke the C function with the allocated pointer.
    let result: i32 = libc.call("stat")
      .arg(Value::CString(b"/etc/hostname".to_vec()))
      .arg(statMem.asPointer())
      .result()?;

    if result != 0 {
      return Err(FFIError::Other("stat() returned non-zero".into()));
    }

    // Read the populated memory block back to the parent.
    let Value::RawString(bytes) = statMem.read()? else {
      return Err(FFIError::Other("expected bytes".into()))
    };
    drop(statMem);

    // Parse the raw bytes into a strongly-typed Rust integer (st_size — offset 48).
    Ok(i64::from_ne_bytes(bytes[48..56].try_into().unwrap()))
  })().expect("stat() via retained scope failed");

  //
  println!("file size = {} bytes", size);

  let expected: u64 = std::fs::metadata("/etc/hostname").expect("metadata").len();
  assert_eq!(size as u64, expected);
  println!("ok: stat via libc through retained FFIScope");
}

// =================================================================================================