use std::sync::Once;
use chillffi::ffi::value::{Type, Value};
use chillffi::setupZygote;
use chillffi::ffi::library::{load, Library};
// =================================================================================================

static Init: Once = Once::new();

fn setup() -> ()
{
  Init.call_once(|| {
    setupZygote().expect("Failed to setup zygote");
  });
}

#[test]
fn testSqrt() -> ()
{
  setup();
  
  let libm: Library = load("libm.so.6").expect("Failed to load library");
  
  let args: Vec<Value> = vec![Value::F64(4.0)];
  let result: Value = libm.call("sqrt", args, Type::F64).unwrap();

  if let Value::F64(val) = result {
    assert!((val - 2.0).abs() < f64::EPSILON);
  } else {
    panic!();
  }
}

#[test]
fn testAbs() -> ()
{
  setup();
  
  let libm: Library = load("libm.so.6").expect("Failed to load library");
  
  let args: Vec<Value> = vec![Value::I32(-5)];
  let result: Value = libm.call("abs", args, Type::I32).unwrap();

  if let Value::I32(val) = result {
    assert_eq!(val, 5);
  } else {
    panic!();
  }
}

// =================================================================================================