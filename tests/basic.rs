use chill_ffi::worker::callExternal;
use chill_ffi::types::{FFIValue, FFIType};
use std::sync::Once;
use chill_ffi::zygote::initZygote;

static Init: Once = Once::new();

fn setup()
{
  Init.call_once(|| {
    initZygote().expect("Failed to init zygote");
  });
}

#[test]
fn test_sqrt()
{
  setup();

  let args = vec![FFIValue::F64(4.0)];
  let result = callExternal(
    "libm.so.6",
    "sqrt",
    args,
    FFIType::F64,
  ).expect("FFI call failed");

  match result
  {
    FFIValue::F64(val) => assert!((val - 2.0).abs() < f64::EPSILON),
    _ => panic!("Unexpected return type"),
  }
}

#[test]
fn test_abs()
{
  setup();

  let args = vec![FFIValue::I32(-5)];
  let result = callExternal(
    "libm.so.6",
    "abs",
    args,
    FFIType::I32,
  ).expect("FFI call failed");

  match result
  {
    FFIValue::I32(val) => assert_eq!(val, 5),
    _ => panic!("Unexpected return type"),
  }
}