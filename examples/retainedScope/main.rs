#[path = "../platform/mod.rs"]
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

/// `FFIScope::enter` — same as `ffi!`, but you control the scope lifetime yourself.
fn main() -> ()
{
  retainedScopeAcrossMultipleOperations();
}

// =================================================================================================

/// One scope reused across several operations.
fn retainedScopeAcrossMultipleOperations() -> ()
{
  let size: i64 = (|| -> Result<i64, FFIError> {
    let ffiScope: FFIScope = FFIScope::enter()?;
    let scope: Scope<'_> = ffiScope.scope();

    let libc: Library = scope.load(LibcPath)?;

    let statMem: AllocatedMemory = scope.alloc(StatSize)?;
    let result: i32 =
      libc.call(StatSymbolName)
        .arg(EtcHostnameCString)
        .arg(statMem.asPointer())
        .result()?;
    if result != 0 {
      return Err(FFIError::Other("stat() returned non-zero".into()));
    }

    let bytes: Vec<u8> = statMem.read()?;
    drop(statMem);

    Ok(i64::from_ne_bytes(bytes[StSizeOffset..StSizeOffset + 8].try_into().unwrap()))
  })().expect("stat() via retained scope failed");

  println!("file size = {size} bytes");

  let expected: u64 = std::fs::metadata(EtcHostnameString).expect("metadata").len();
  assert_eq!(size as u64, expected);
  println!("ok: stat via libc through retained FFIScope");
}

// =================================================================================================
