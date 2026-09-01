use chillffi::ffi;
// =================================================================================================

/// Execute math functions from libm
fn main() -> ()
{
  // Call sqrt(4.0)
  let result: f64 = ffi!(|scope| {
    let libm: Library = scope.load("libm.so.6")?;
    Ok( libm.call("sqrt").arg::<f64>(4.0).result()? )
  }).expect("FFI call failed");

  println!("sqrt(4.0) = {}", result);
  assert!((result - 2.0).abs() < f64::EPSILON, "sqrt(4.0) != 2.0");

  // Call abs(-5)
  let result: i32 = ffi!(|scope| {
    let libm: Library = scope.load("libm.so.6")?;
    Ok( libm.call("abs").arg::<i32>(-5).result()? )
  }).expect("FFI call failed");

  println!("abs(-5) = {}", result);
  assert_eq!(result, 5, "abs(-5) != 5");

  //
  println!("All tests passed!");
}

// =================================================================================================