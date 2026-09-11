mod platform;
use crate::platform::StatSymbolName;
use crate::platform::{EtcHostnameCString, EtcHostnameString, LibcPath, StSizeOffset, StatSize};
// =================================================================================================
use chillffi::ffi;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::errors::FFIError;
// =================================================================================================

/// Call stat() via libc and read file size from struct out-parameter.
fn main() -> ()
{
  let size: i64 = ffi!(|scope| {
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
  }).expect("stat() failed");

  //
  println!("file size = {} bytes", size);

  let expected: u64 = std::fs::metadata(EtcHostnameString).expect("metadata").len();
  assert_eq!(size as u64, expected);
  println!("ok: stat via libc");
}

// =================================================================================================