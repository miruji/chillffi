use chillffi::callv;
use chillffi::ffi::allocatedMemory::{AllocatedMemory};
use chillffi::ffi::value::{Value};
use chillffi::ffi::errors::FFIError;
use chillffi::ffi;
// =================================================================================================

fn main() -> ()
{
  // clock_gettime(CLOCK_REALTIME, &timespec) — struct out-param via Alloc/ReadMemory,
  // the case a plain Value::Pointer can't cover on its own.
  let (secs, nanos): (i64, i64) = ffi!(|scope| {
    let libc: Library = Library::load("libc.so.6")?;

    // struct timespec { time_t tv_sec; long tv_nsec; } — 16 bytes on x86_64 Linux
    let mem: AllocatedMemory = scope.alloc(16)?;

    callv!(libc, "clock_gettime", 0 as i32 /* CLOCK_REALTIME */, mem.asPointer())?;

    let Value::RawString(bytes) = mem.read()? else { 
      return Err(FFIError::Other("expected bytes".into())) 
    };
    drop(mem);

    let secs: i64 = i64::from_ne_bytes(bytes[0..8].try_into().unwrap());
    let nanos: i64 = i64::from_ne_bytes(bytes[8..16].try_into().unwrap());
    Ok((secs, nanos))
  }).expect("clock_gettime failed");

  println!("clock_gettime(CLOCK_REALTIME) = {}.{:09}", secs, nanos);
}

// =================================================================================================