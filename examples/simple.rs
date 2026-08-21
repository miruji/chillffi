use chillffi::ffi::value::{Type, Value};
use chillffi::ffi;
// =================================================================================================

fn main() -> ()
{
  // Test 1: Call sqrt(4.0) from libm.so
  let result: Value = ffi!{
    let libm: Library = Library::load("libm.so.6")?;
    let args: Vec<Value> = vec![Value::F64(4.0)];
    Ok(libm.call("sqrt", args, Type::F64)?)
  }.expect("FFI call failed");

  match result
  {
    Value::F64(val) =>
    {
      println!("sqrt(4.0) = {}", val);
      assert!((val - 2.0).abs() < f64::EPSILON, "sqrt(4.0) != 2.0");
    }
    _ => panic!("Unexpected return type for sqrt"),
  }

  // Test 2: Call abs(-5) from libm.so
  let result: Value = ffi!{
    let libm: Library = Library::load("libm.so.6")?;
    let args: Vec<Value> = vec![Value::I32(-5)];
    Ok(libm.call("abs", args, Type::I32)?)
  }.expect("FFI call failed");

  match result
  {
    Value::I32(val) =>
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