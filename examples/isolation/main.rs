#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::LibmPath;
use crate::platform::platformExt;
// =================================================================================================
use chillffi::ffi;
use chillffi::ffi::errors::FFIError;
// =================================================================================================

/// Crash isolation: a crash inside the clone must not take down the main process.
fn main() -> ()
{
  segfaultIsContained();
  abortIsContained();
  runtimeSurvivesAfterCrash();
}

// =================================================================================================

/// SIGSEGV inside the clone → `Err`, main process stays up.
fn segfaultIsContained() -> ()
{
  let result: Result<(), FFIError> = ffi!(|scope| {
    scope.addSearchPath("examples/isolation");
    let lib: Library = scope.load(platformExt!("libcrash"))?;
    lib.call("triggerSegfault").void()
  });

  let err: FFIError = result.expect_err("a segfaulting clone must not report success");
  assert!(matches!(err, FFIError::ZygoteCommunicationFailed(_)), "unexpected error: {err:?}");
  println!("ok: segfault contained — {err}");
}

/// Same for `abort()` (SIGABRT).
fn abortIsContained() -> ()
{
  let result: Result<(), FFIError> = ffi!(|scope| {
    scope.addSearchPath("examples/isolation");
    let lib: Library = scope.load(platformExt!("libcrash"))?;
    lib.call("triggerAbort").void()
  });

  let err: FFIError = result.expect_err("an aborting clone must not report success");
  assert!(matches!(err, FFIError::ZygoteCommunicationFailed(_)), "unexpected error: {err:?}");
  println!("ok: abort() contained — {err}");
}

/// After two crashed clones, a normal `ffi!` block still works.
fn runtimeSurvivesAfterCrash() -> ()
{
  let result: f64 = ffi!(|scope| {
    let libm: Library = scope.load(LibmPath)?;
    libm.call("sqrt").arg::<f64>(16.0).result()
  }).expect("runtime should still work after two crashed clones");

  assert!((result - 4.0).abs() < f64::EPSILON);
  println!("ok: runtime alive after crashes, sqrt(16) = {result}");
}

// =================================================================================================
