use chill_ffi::tokenizer::{Token, TokenType};
use chill_ffi::parser::StructureType;
use chill_ffi::worker::callExternal;
use chill_ffi::types::FFIValue;
use std::sync::Once;

static INIT: Once = Once::new();

fn setup()
{
  INIT.call_once(|| {
    chill_ffi::zygote::initZygote().expect("Failed to init zygote");
  });
}

#[test]
fn test_sqrt()
{
  setup();

  let mut arg_token = Token::new("4.0".to_string(), TokenType::Float);
  let result = callExternal(
    "libm.so.6",
    "sqrt",
    &mut [arg_token],
    StructureType::F64,
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

  let mut arg_token = Token::new("5".to_string(), TokenType::Int);
  let result = callExternal(
    "libm.so.6",
    "abs",
    &mut [arg_token],
    StructureType::I32,
  ).expect("FFI call failed");

  match result
  {
    FFIValue::I32(val) => assert_eq!(val, 5),
    _ => panic!("Unexpected return type"),
  }
}