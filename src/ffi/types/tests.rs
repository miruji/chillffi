
// =================================================================================================

#[cfg(test)]
mod tests
{
  use crate::ffi;
  use std::ffi::CString;
  use crate::ffi::types::{Pointer, Value};
  // ===============================================================================================

  /// Checks all signed integer types 
  ///
  /// [`Value::I8`], [`Value::I16`], [`Value::I32`], [`Value::I64`], [`Value::Isize`]
  #[test]
  fn signedIntegers() -> ()
  {
    ffi!(|scope| {
      let libc: Library = scope.load("libc.so.6")?;
      
      let resI8: i8 = libc.call("abs").arg::<i8>(-5).result()?;
      assert!(matches!(resI8, 5));
      
      let resI16: i16 = libc.call("abs").arg::<i16>(-15).result()?;
      assert!(matches!(resI16, 15));
      
      let resI32: i32 = libc.call("abs").arg::<i32>(-42).result()?;
      assert!(matches!(resI32, 42));
      
      let resI64: i64 = libc.call("labs").arg::<i64>(-100000).result()?;
      assert!(matches!(resI64, 100000));
      
      let resIsize: isize = libc.call("labs").arg::<isize>(-500).result()?;
      assert!(matches!(resIsize, 500));

      Ok(())
    }).expect("Signed integers test failed");
  }

  /// Checks all unsigned integer types
  ///
  /// [`Value::U8`], [`Value::U16`], [`Value::U32`], [`Value::U64`], [`Value::Usize`].
  #[test]
  fn unsignedIntegers() -> ()
  {
    ffi!(|scope| {
      let libc: Library = scope.load("libc.so.6")?;
      
      let resU8: u8 = 
        libc.call("strnlen")
          .arg(c"a")
          .arg::<u8>(10)
          .result()?;
      assert!(matches!(resU8, 1));
      
      let resU16: u16 = 
        libc.call("strnlen")
        .arg(c"ab")
        .arg::<u16>(10)
        .result()?;
      assert!(matches!(resU16, 2));
      
      let resU32: u32 = 
        libc.call("strnlen")
        .arg(c"abc")
        .arg::<u32>(10)
        .result()?;
      assert!(matches!(resU32, 3));
      
      let resU64: u64 = 
        libc.call("strnlen")
        .arg(c"abcd")
        .arg::<u64>(10)
        .result()?;
      assert!(matches!(resU64, 4));
      
      let resUsize: usize = 
        libc.call("strnlen")
        .arg(c"abcde")
        .arg::<usize>(10)
        .result()?;
      assert!(matches!(resUsize, 5));

      Ok(())
    }).expect("Unsigned integers test failed");
  }

  /// Checks passing floating point numbers
  ///
  /// [`Value::F32`], [`Value::F64`].
  #[test]
  fn float() -> ()
  {
    let resultF32: f32 = ffi!(|scope| {
      let libm: Library = scope.load("libm.so.6")?;
      Ok( libm.call("sqrtf").arg::<f32>(16.0).result()? )
    }).expect("FFI F32 call failed");

    assert!((resultF32 - 4.0).abs() < f32::EPSILON);

    let resultF64: f64 = ffi!(|scope| {
      let libm: Library = scope.load("libm.so.6")?;
      Ok(
        libm.call("pow")
          .arg::<f64>(2.0)
          .arg::<f64>(3.0)
          .result()?
      )
    }).expect("FFI F64 call failed");

    assert!((resultF64 - 8.0).abs() < f64::EPSILON);
  }

  // ===============================================================================================

  /// Checks passing [`Value::Bool`].
  #[test]
  fn bool() -> ()
  {
    let result: bool = ffi!(|scope| {
      let libc: Library = scope.load("libc.so.6")?;
      Ok( libc.call("isalpha").arg(true).result()? )
    }).expect("FFI Bool call failed");

    assert!(!result);
  }

  // ===============================================================================================

  /// Checks pointer type handling: passing a valid pointer 
  /// and receiving NULL for a missing variable.
  #[test]
  fn pointer() -> ()
  {
    let result: Pointer = ffi!(|scope| {
      let libc: Library = scope.load("libc.so.6")?;
      Ok(
        libc.call("getenv")
          .arg(c"noSuchVar")
          .result()?
      )
    }).expect("FFI pointer call failed");

    assert_eq!(result, Pointer(0));
  }

  /// Checks that a pointer returned inside one ffi!{} block stays valid for reuse as an argument
  /// within the same block (same clone process, same address space).
  ///
  /// Env vars set in the host are invisible to the clone — its environ was captured at zygote
  /// startup (before main()). strdup() sidesteps this: it allocates directly in the clone's own
  /// heap, so the round-trip is verified without relying on inherited process state.
  #[test]
  fn pointerRoundtrip() -> ()
  {
    let len: usize = ffi!(|scope| {
      let libc: Library = scope.load("libc.so.6")?;
      let ptr: Pointer = 
        libc.call("strdup")
          .arg(c"hello")
          .result()?;
      assert_ne!(ptr, Pointer(0));
  
      let result: usize = libc.call("strlen").arg(ptr).result()?;
      libc.call("free").arg(ptr).void()?;
      Ok(result)
    }).expect("pointer roundtrip failed");

    assert!(matches!(len, 5));
  }

  // ===============================================================================================

  /// Checks string conversion bridges and `TryFrom` validation.
  #[test]
  fn stringBridges() -> ()
  {
    let valString: Value = String::from("hello").into();
    assert!(matches!(valString, Value::String(_)));

    let valStr: Value = "hello".into();
    assert_eq!(valStr, Value::String(b"hello".to_vec()));

    let valCString: Value = CString::new("hello").unwrap().into();
    assert_eq!(valCString, Value::CString(b"hello".to_vec()));

    let valRawBytes: Value = vec![0u8, 1, 2].into();
    assert_eq!(valRawBytes, Value::RawString(vec![0, 1, 2]));

    // Reverse conversions
    let resString: String = Value::RawString(b"hello".to_vec()).try_into().unwrap();
    assert_eq!(resString, "hello");

    assert!(CString::try_from(Value::RawString(b"with\0nul".to_vec())).is_err());
  }

  /// Checks passing CString to a C function expecting a \0-terminated string (strlen).
  #[test]
  fn cString() -> ()
  {
    let result: usize = ffi!(|scope| {
      let libc: Library = scope.load("libc.so.6")?;
      Ok(
        libc.call("strlen")
          .arg(c"hello")
          .result()?
      )
    }).expect("FFI CString call failed");

    assert_eq!(result, 5);
  }

  /// Checks passing String which automatically expands to two C-ABI arguments (ptr + len)
  /// for functions accepting buffer pointer and max length (strnlen).
  #[test]
  fn string() -> ()
  {
    let result: usize = ffi!(|scope| {
      let libc: Library = scope.load("libc.so.6")?;
      Ok(
        libc.call("strnlen")
          .arg("hello world")
          .result()?
      )
    }).expect("FFI String call failed");

    assert_eq!(result, 11);
  }

  /// Checks passing RawString as a single raw byte pointer (atoi).
  #[test]
  fn rawString() -> ()
  {
    let result: i32 = ffi!(|scope| {
      let libc: Library = scope.load("libc.so.6")?;
      Ok(
        libc.call("atoi")
          .arg(b"12345\0".to_vec())
          .result()?
      )
    }).expect("FFI RawString call failed");

    assert_eq!(result, 12345);
  }

  // ===============================================================================================
}

// =================================================================================================