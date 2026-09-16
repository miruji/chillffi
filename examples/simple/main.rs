#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::LibmPath;
// =================================================================================================
use chillffi::ffi;
// =================================================================================================

/// Minimal `ffi!` block: one library, one call.
fn main() -> ()
{
  sqrt();
  multipleCallsSameScope();
}

// =================================================================================================

/// sqrt from libm.
fn sqrt() -> ()
{
  let result: f64 = ffi!(|scope| {
    let libm: Library = scope.load(LibmPath)?;
    libm.call("sqrt").arg::<f64>(4.0).result()
  }).expect("FFI call failed");

  assert!((result - 2.0).abs() < f64::EPSILON, "sqrt(4.0) != 2.0");
  println!("ok: sqrt(4.0) = {result}");
}

/// Second independent `ffi!` block — a separate clone.
fn multipleCallsSameScope() -> ()
{
  let result: i32 = ffi!(|scope| {
    let libm: Library = scope.load(LibmPath)?;
    libm.call("abs").arg::<i32>(-5).result()
  }).expect("FFI call failed");

  assert_eq!(result, 5, "abs(-5) != 5");
  println!("ok: abs(-5) = {result}");
}

// =================================================================================================
