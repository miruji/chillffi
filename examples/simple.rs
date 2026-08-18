use chill_ffi::tokenizer::{Token, TokenType};
use chill_ffi::parser::StructureType;
use chill_ffi::worker::callExternal;
use chill_ffi::types::FFIValue;

fn main()
{
  // Инициализируем зиготу (обязательно первой строкой)
  chill_ffi::zygote::initZygote().expect("Failed to init zygote");

  // Тест 1: Вызов sqrt(4.0) из libm.so
  let mut arg_token = Token::new("4.0".to_string(), TokenType::Float);
  let result = callExternal(
    "libm.so.6",  // На Ubuntu/Debian. На других системах: "libm.so" или "/usr/lib/x86_64-linux-gnu/libm.so.6"
    "sqrt",
    &mut [arg_token],
    StructureType::F64,
  ).expect("FFI call failed");

  match result
  {
    FFIValue::F64(val) =>
      {
        println!("sqrt(4.0) = {}", val);
        assert!((val - 2.0).abs() < f64::EPSILON, "sqrt(4.0) != 2.0");
      }
    _ => panic!("Unexpected return type for sqrt"),
  }

  // Тест 2: Вызов abs(-5) из libm.so
  let mut arg_token = Token::new("5".to_string(), TokenType::Int);
  let result = callExternal(
    "libm.so.6",
    "abs",
    &mut [arg_token],
    StructureType::I32,
  ).expect("FFI call failed");

  match result
  {
    FFIValue::I32(val) =>
      {
        println!("abs(-5) = {}", val);
        assert_eq!(val, 5, "abs(-5) != 5");
      }
    _ => panic!("Unexpected return type for abs"),
  }

  println!("All tests passed!");
}