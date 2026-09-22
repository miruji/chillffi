#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::platformExt;
// =================================================================================================
use chillffi::errnoPolicy::setGlobalReadErrno;
use chillffi::ffi;
use chillffi::ffi::scope::Scope;
// =================================================================================================

/// Errno capture: per-call, per-scope, and global. Priority: call > scope > global.
///
/// On Windows there is a second error channel — `GetLastError()` — captured
/// under the **same** `.errno()` / `setReadErrno` flag as CRT errno. Read it
/// with [`Scope::lastOsError`]. There is no separate builder method: one flag
/// controls both channels. On Unix `lastOsError()` is always `None`.
fn main() -> ()
{
  callErrno();
  scopeErrno();
  globalErrno();
  #[cfg(windows)]
  callOsError();
}

// =================================================================================================

/// `.errno()` on a single call → CRT errno via [`Scope::lastErrno`].
fn callErrno() -> ()
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
fn scopeErrno() -> ()
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
fn globalErrno() -> ()
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

/// Windows: same `.errno()` also captures `GetLastError` → [`Scope::lastOsError`].
///
/// No extra builder method — the `readErrno` flag covers both channels.
#[cfg(windows)]
fn callOsError() -> ()
{
  const ErrorAccessDenied: u32 = 5; // ERROR_ACCESS_DENIED

  let osError: Option<u32> = ffi!(|scope| {
    scope.addSearchPath("examples/errno");
    let liberrno: Library = scope.load(platformExt!("liberrno"))?;

    let result: i32 =
      liberrno.call("failWithOsError")
        .arg::<u32>(ErrorAccessDenied)
        .errno()
        .result()?;

    assert_eq!(result, -1);
    Ok(Scope::lastOsError())
  }).expect("call-level osError failed");

  assert_eq!(osError, Some(ErrorAccessDenied));
  println!("ok: call-level lastOsError (GetLastError) = {osError:?}");
}

// =================================================================================================
