use chillffi::ffi::library::{load, Library};
use chillffi::ffi::value::{Type, Value};
use chillffi::setupZygote;
// =================================================================================================

fn main() -> ()
{
  // Если запущен как зигота, переключаемся в режим обработки запросов
  setupZygote().expect("Failed to setup zygote");

  // Загружаем библиотеку libm.so
  let libm: Library = load("libm.so.6").expect("Failed to load library");

  // Тест 1: Вызов sqrt(4.0) из libm.so
  let args: Vec<Value> = vec![Value::F64(4.0)];
  let result: Value = libm.call(
    "sqrt",
    args,
    Type::F64,
  ).expect("FFI call failed");

  match result
  {
    Value::F64(val) =>
      {
        println!("sqrt(4.0) = {}", val);
        assert!((val - 2.0).abs() < f64::EPSILON, "sqrt(4.0) != 2.0");
      }
    _ => panic!("Unexpected return type for sqrt"),
  }

  // Тест 2: Вызов abs(-5) из libm.so
  let args: Vec<Value> = vec![Value::I32(-5)];
  let result: Value = libm.call(
    "abs",
    args,
    Type::I32,
  ).expect("FFI call failed");

  match result
  {
    Value::I32(val) =>
      {
        println!("abs(-5) = {}", val);
        assert_eq!(val, 5, "abs(-5) != 5");
      }
    _ => panic!("Unexpected return type for abs"),
  }

  // Выгружаем библиотеку
  libm.unload().expect("Failed to unload library");

  //
  println!("All tests passed!");
}

// =================================================================================================