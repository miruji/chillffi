#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::{SprintfLibPath, SprintfSymbolName};
// =================================================================================================
use chillffi::ffi;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::errors::FFIError;
// =================================================================================================

/// Variadic FFI calls via [`CallBuilder::variadic`].
fn main() -> ()
{
  sprintfVariadic();
  variadicWithoutFixedArgsFails();
}

// =================================================================================================

/// `sprintf(char *str, const char *format, ...)` — two fixed args, then variadic.
fn sprintfVariadic() -> ()
{
  let text: String = ffi!(|scope| {
    let libc: Library = scope.load(SprintfLibPath)?;
    let mem: AllocatedMemory = scope.alloc(64)?;

    let written: i32 = libc.call(SprintfSymbolName)
      .arg(mem.asPointer()) // char *str       — fixed 1
      .arg(c"Hello %s %d!") // const char *fmt — fixed 2
      .variadic()           // <- everything after this is variadic
      .arg(c"world")        // %s
      .arg::<i32>(42)       // %d
      .result()?;

    // mem.read() returns the whole allocation (junk after NUL).
    // sprintf's return value is the exact length of the formatted string.
    let bytes: Vec<u8> = mem.read()?;
    let text: String = String::from_utf8(bytes[..written as usize].to_vec())
      .expect("sprintf output must be valid UTF-8");

    Ok(text)
  }).expect("variadic sprintf failed");

  assert_eq!(text, "Hello world 42!");
  println!("ok: sprintf(...) = {text:?}");
}

/// `.variadic()` with no fixed arguments is rejected (`nfixedargs >= 1`).
fn variadicWithoutFixedArgsFails() -> ()
{
  let err: FFIError = ffi!(|scope| {
    let libc: Library = scope.load(SprintfLibPath)?;
    libc.call(SprintfSymbolName)
      .variadic()
      .arg(c"hello")
      .result::<i32>()
  }).expect_err("a variadic call without fixed arguments should fail");

  assert!(matches!(err, FFIError::BadArgument(_)), "unexpected error: {err:?}");
  println!("ok: variadic without fixed args -> BadArgument");
}

// =================================================================================================
