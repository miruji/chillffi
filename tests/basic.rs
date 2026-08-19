use std::sync::Once;
use chillffi::ffi::value::{Type, Value};
use chillffi::{ffi, setupZygote};
// =================================================================================================

static Init: Once = Once::new();

/// todo desc
fn setup() -> ()
{
  Init.call_once(|| {
    setupZygote().expect("Failed to setup zygote");
  });
}

// =================================================================================================

/// todo desc
#[test]
fn testSqrt() -> ()
{
  setup();

  let result: Value = ffi!{
    let libm = Library::load("libm.so.6")?;
    let args = vec![Value::F64(4.0)];
    Ok(libm.call("sqrt", args, Type::F64)?)
  }.expect("FFI call failed");

  if let Value::F64(val) = result {
    assert!((val - 2.0).abs() < f64::EPSILON);
  } else {
    panic!("Expected F64");
  }
}

/// todo desc
#[test]
fn testAbs() -> ()
{
  setup();
  
  let result: Value = ffi! {
    let libm = Library::load("libm.so.6")?;
    let args = vec![Value::I32(-5)];
    Ok(libm.call("abs", args, Type::I32)?)
  }.expect("FFI call failed");

  if let Value::I32(val) = result {
    assert_eq!(val, 5);
  } else {
    panic!("Expected I32");
  }
}

// =================================================================================================