#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::LibmPath;
use crate::platform::platformExt;
// =================================================================================================
use chillffi::ffi;
use chillffi::ffi::errors::FFIError;
// =================================================================================================

/// `Library`: load, path, unload, and load/call failures.
fn main() -> ()
{
  loadCallAndPath();
  explicitUnload();
  automaticDropOnScopeExit();
  libraryLoadFailed();
  symbolNotFound();
}

// =================================================================================================

/// `load` only stores the path; `path()` returns it as-is.
fn loadCallAndPath() -> ()
{
  let (path, result): (String, f64) = ffi!(|scope| {
    let libm: Library = scope.load(LibmPath)?;
    let path: String = libm.path().to_string();
    let result: f64 = libm.call("sqrt").arg::<f64>(16.0).result()?;
    Ok((path, result))
  }).expect("load/call/path failed");

  assert_eq!(path, LibmPath);
  assert!((result - 4.0).abs() < f64::EPSILON);
  println!("ok: loaded '{path}', sqrt(16.0) = {result}");
}

/// `unload` takes `self` — the handle cannot be used afterward.
fn explicitUnload() -> ()
{
  ffi!(|scope| {
    let libm: Library = scope.load(LibmPath)?;
    let result: f64 = libm.call("sqrt").arg::<f64>(9.0).result()?;
    assert!((result - 3.0).abs() < f64::EPSILON);
    libm.unload()?;
    // `libm` is gone here — any further use would be a compile error, not a panic.
    Ok(())
  }).expect("explicit unload failed");

  println!("ok: explicit unload()");
}

/// Without an explicit `unload`, the library is dropped when the scope ends.
fn automaticDropOnScopeExit() -> ()
{
  ffi!(|scope| {
    let libm: Library = scope.load(LibmPath)?;
    let result: f64 = libm.call("sqrt").arg::<f64>(25.0).result()?;
    assert!((result - 5.0).abs() < f64::EPSILON);
    Ok(())
    // `libm` drops here.
  }).expect("automatic drop failed");

  println!("ok: automatic drop on scope exit");
}

/// Bad path: `load` succeeds, the error arrives on the first real call.
fn libraryLoadFailed() -> ()
{
  let err: FFIError = ffi!(|scope| {
    let bogus: Library = scope.load(platformExt!("libTotallyDoesNotExist9000"))?;
    bogus.call("whatever").void()
  }).expect_err("loading a nonexistent library should fail");

  assert!(matches!(err, FFIError::LibraryLoadFailed{ .. }), "unexpected error: {err:?}");
  println!("ok: LibraryLoadFailed — {err}");
}

/// Library loads fine, symbol is missing.
fn symbolNotFound() -> ()
{
  let err: FFIError = ffi!(|scope| {
    let libm: Library = scope.load(LibmPath)?;
    libm.call("thisSymbolDoesNotExistAnywhere").void()
  }).expect_err("calling a missing symbol should fail");

  assert!(matches!(err, FFIError::SymbolNotFound{ .. }), "unexpected error: {err:?}");
  println!("ok: SymbolNotFound — {err}");
}

// =================================================================================================
