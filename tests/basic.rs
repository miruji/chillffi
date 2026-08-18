use std::sync::Once;
use chillffi::setupZygote;
use chillffi::worker::callExternal;
use chillffi::types::{FFIValue, FFIType};
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
  
  let args: Vec<FFIValue> = vec![FFIValue::F64(4.0)];
  let result: FFIValue = callExternal("libm.so.6", "sqrt", args, FFIType::F64).unwrap();
  
  if let FFIValue::F64(val) = result {
    assert!((val - 2.0).abs() < f64::EPSILON);
  } else {
    panic!();
  }
}

#[test]
fn testAbs() -> ()
{
  setup();
  
  let args: Vec<FFIValue> = vec![FFIValue::I32(-5)];
  let result: FFIValue = callExternal("libm.so.6", "abs", args, FFIType::I32).unwrap();
  
  if let FFIValue::I32(val) = result {
    assert_eq!(val, 5);
  } else {
    panic!();
  }
}

// =================================================================================================