#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::platformExt;
// =================================================================================================
use chillffi::errnoPolicy::setGlobalReadErrno;
use chillffi::ffi;
use chillffi::ffi::scope::Scope;
// =================================================================================================

/// Errno capture: per-call, per-scope, and global. Priority: call > scope > global.
fn main() -> ()
{
  testCallErrno();
  testScopeErrno();
  testGlobalErrno();
}

// =================================================================================================

/// `.errno()` on a single call.
fn testCallErrno() -> ()
{
  let errno: Option<i32> = ffi!(|scope| {
    scope.addSearchPath("examples/errno");
    let liberrno: Library = scope.load(platformExt!("liberrno"))?;

    let result: i32 =
      liberrno.call("failWithErrno")
        .arg::<i32>(2 /* ENOENT */)
        .errno()
        .result()?;

    assert_eq!(result, -1);
    Ok(Scope::lastErrno())
  }).expect("call-level errno failed");

  assert_eq!(errno, Some(2));
  println!("ok: call-level errno");
}

/// `setReadErrno(true)` on the scope — every call in the block captures errno.
fn testScopeErrno() -> ()
{
  let errno: Option<i32> = ffi!(|scope| {
    scope.addSearchPath("examples/errno");
    scope.setReadErrno(true);
    let liberrno: Library = scope.load(platformExt!("liberrno"))?;

    let result: i32 =
      liberrno.call("failWithErrno")
        .arg::<i32>(4 /* EINTR */)
        .result()?; // no .errno() — inherits the scope default

    assert_eq!(result, -1);
    Ok(Scope::lastErrno())
  }).expect("scope-level errno failed");

  assert_eq!(errno, Some(4));
  println!("ok: scope-level errno");
}

/// Global `setGlobalReadErrno(true)`.
fn testGlobalErrno() -> ()
{
  setGlobalReadErrno(true);

  let errno: Option<i32> = ffi!(|scope| {
    scope.addSearchPath("examples/errno");
    let liberrno: Library = scope.load(platformExt!("liberrno"))?;

    let result: i32 =
      liberrno.call("failWithErrno")
        .arg::<i32>(9 /* EBADF */)
        .result()?; // no .errno(), no scope override — inherits the global default

    assert_eq!(result, -1);
    Ok(Scope::lastErrno())
  }).expect("global-level errno failed");

  assert_eq!(errno, Some(9));
  println!("ok: global-level errno");
}

// =================================================================================================
