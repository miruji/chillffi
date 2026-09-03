use chillffi::errnoPolicy::setGlobalReadErrno;
use chillffi::ffi::scope::Scope;
use chillffi::ffi;
// =================================================================================================

/// Test errno capture using a per-call override, a scope-level default,
/// and a global default — mirroring `examples/paths/main.rs`'s raw/scope/
/// global levels for path resolution. Most specific wins: call > scope > global.
fn main() -> ()
{
  testCallErrno();
  testScopeErrno();
  testGlobalErrno();
}

/// Explicit per-call override via `.errno()` — captured regardless of any
/// scope or global default (both are still off at this point).
fn testCallErrno() -> ()
{
  let errno: Option<i32> = ffi!(|scope| {
    scope.addSearchPath("examples/errno");
    let liberrno: Library = scope.load("liberrno.so")?;

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

/// Scope-level default via `Scope::setReadErrno(true)` — every call made
/// through this scope captures errno without needing its own `.errno()`.
fn testScopeErrno() -> ()
{
  let errno: Option<i32> = ffi!(|scope| {
    scope.addSearchPath("examples/errno");
    scope.setReadErrno(true);
    let liberrno: Library = scope.load("liberrno.so")?;

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

/// Global default via `setGlobalReadErrno(true)` — set once, applies to
/// every call in every scope from here on, unless a scope or call overrides it.
fn testGlobalErrno() -> ()
{
  setGlobalReadErrno(true);

  let errno: Option<i32> = ffi!(|scope| {
    scope.addSearchPath("examples/errno");
    let liberrno: Library = scope.load("liberrno.so")?;

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