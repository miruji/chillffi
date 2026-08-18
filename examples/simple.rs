use chillffi::setupZygote;
use chillffi::worker::callExternal;
use chillffi::types::{FFIValue, FFIType};
// =================================================================================================

fn main() -> ()
{
  // Если запущен как зигота, переключаемся в режим обработки запросов
  setupZygote().expect("Failed to setup zygote");

  // Тест 1: Вызов sqrt(4.0) из libm.so
  let args: Vec<FFIValue> = vec![FFIValue::F64(4.0)];
  let result: FFIValue = callExternal(
    "libm.so.6",  // На Ubuntu/Debian. На других системах: "libm.so" или "/usr/lib/x86_64-linux-gnu/libm.so.6"
    "sqrt",
    args,
    FFIType::F64,
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
  let args: Vec<FFIValue> = vec![FFIValue::I32(-5)];
  let result: FFIValue = callExternal(
    "libm.so.6",
    "abs",
    args,
    FFIType::I32,
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

  //
  println!("All tests passed!");
}

// =================================================================================================