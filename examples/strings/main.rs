#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::LibcPath;
// =================================================================================================
use chillffi::ffi;
// =================================================================================================

/// Three string argument kinds: String, CString, RawString.
fn main() -> ()
{
  string();
  cString();
  rawString();
}

// =================================================================================================

/// `String`/`&str` → pointer + length.
fn string() -> ()
{
  let result: usize = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    libc.call("strnlen").arg("hello world").result()
  }).expect("String call failed");

  assert_eq!(result, 11);
  println!("ok: strnlen(\"hello world\") = {result}  (auto-split into 2 C args: pointer + len)");
}

/// `CString`/`&CStr` → one `\0`-terminated pointer.
fn cString() -> ()
{
  let result: usize = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    libc.call("strlen").arg(c"chillffi").result()
  }).expect("CString call failed");

  assert_eq!(result, 8);
  println!("ok: strlen(c\"chillffi\") = {result}  (1 C arg: pointer)");
}

/// `Vec<u8>`/`&[u8]` → raw pointer.
fn rawString() -> ()
{
  let result: i32 = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    libc.call("atoi").arg(b"12345\0".to_vec()).result()
  }).expect("RawString call failed");

  assert_eq!(result, 12345);
  println!("ok: atoi(b\"12345\\0\") = {result}  (1 C arg: pointer, no auto length/NUL)");
}

// =================================================================================================
