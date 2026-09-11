mod platform;
use crate::platform::EtcHostnameCString;
use crate::platform::EtcHostnameString;
use crate::platform::LibcPath;
use crate::platform::StSizeOffset;
use crate::platform::StatSize;
use crate::platform::StatSymbolName;
// =================================================================================================
use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::errors::FFIError;
use chillffi::ffi::library::Library;
use chillffi::ffi::scope::{FFIScope, Scope};
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

    //
    let libc: Library = scope.load(LibcPath)?;

    // Allocate memory for the out-parameter.
    let statMem: AllocatedMemory = scope.alloc(StatSize)?;

    // Call stat() with path and allocated buffer.
    let result: i32 = 
      libc.call(StatSymbolName)
        .arg(EtcHostnameCString)
        .arg(statMem.asPointer())
        .result()?;
    if result != 0 {
      return Err(FFIError::Other("stat() returned non-zero".into()));
    }

    // Read the populated memory block back to the parent.
    let bytes: Vec<u8> = statMem.read()?;
    drop(statMem);

    // Parse st_size from raw bytes
    Ok(i64::from_ne_bytes(bytes[StSizeOffset..StSizeOffset+8].try_into().unwrap()))
  })().expect("stat() via retained scope failed");

  //
  println!("file size = {} bytes", size);

  let expected: u64 = std::fs::metadata(EtcHostnameString).expect("metadata").len();
  assert_eq!(size as u64, expected);
  println!("ok: stat via libc through retained FFIScope");
}

// =================================================================================================