use serde::{Deserialize, Serialize};
// =================================================================================================

/// A value that can be passed between processes and used when calling FFI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Value
{
  /// Just an empty value
  None,
  
  //
  U8(u8),
  U16(u16),
  U32(u32),
  U64(u64),
  Usize(usize),
  
  //
  I8(i8),
  I16(i16),
  I32(i32),
  I64(i64),
  Isize(isize),
  
  //
  F32(f32),
  F64(f64),
  
  //
  Bool(bool),

  /// Store the address as a regular number;
  ///
  /// The address is stored as usize rather than as a raw pointer — for two reasons:
  /// serialization through bincode/serde (raw pointers cannot do this)
  /// and the fact that the owner of the memory is the C code on the zygote clone side,
  /// not Rust.
  ///
  /// # Lifetime
  /// Valid strictly within the same ffi!{} block — that is, inside the same
  /// zygote clone. The clone dies when leaving the block
  /// (ZygoteGuard::drop → SIGKILL), and along with it dies the address
  /// space to which this address belonged.
  ///
  /// Using it outside the block is undefined behavior, not an Err:
  /// a new clone is forked from the same parent zygote and often has
  /// the same mapped address space, therefore at that address
  /// there may be unrelated memory instead of the expected crash.
  ///
  /// Ownership and deallocation (free/strdup and similar) are exclusively on
  /// the side of the calling C code.
  Pointer(usize),

  /// In C code, one would expect `uint8_t *data`;
  /// But without `len` these bytes are useless and `size_t len` is necessary.
  RawString(Vec<u8>),

  /// In C code, one would expect `const char *str`; `\0` terminated.
  CString(Vec<u8>),

  /// In C code, one would expect `const char *str, size_t len`.
  String(Vec<u8>)
}

// =================================================================================================

/// Description of the value type 
/// for defining FFI arguments and result.
///
/// todo: add unit tests for Type variants verification
#[derive(Serialize, Deserialize)]
pub enum Type
{
  /// Just an empty value
  None,
  
  //
  U8,
  U16,
  U32,
  U64,
  Usize,
  
  //
  I8,
  I16,
  I32,
  I64,
  Isize,
  
  //
  F32,
  F64,
  
  //
  Bool,
  
  /// Raw pointer
  Pointer
}

// =================================================================================================

#[cfg(test)]
mod tests
{
  use crate::ffi;
  use crate::ffi::value::{Type, Value};
  use crate::ffi::errors::FFIError;
  // ===============================================================================================

  /// Checks all signed integer types (I8, I16, I32, I64, Isize).
  #[test]
  fn signedIntegers() -> ()
  {
    ffi!{
      let libc: Library = Library::load("libc.so.6")?;

      // I8 & I16
      let resI8: Value = libc.call("abs", vec![Value::I8(-5)], Type::I8)?;
      let resI16: Value = libc.call("abs", vec![Value::I16(-15)], Type::I16)?;
      assert!(matches!(resI8, Value::I8(5)));
      assert!(matches!(resI16, Value::I16(15)));

      // I32, I64 & Isize
      let resI32: Value = libc.call("abs", vec![Value::I32(-42)], Type::I32)?;
      let resI64: Value = libc.call("labs", vec![Value::I64(-100000)], Type::I64)?;
      let resIsize: Value = libc.call("labs", vec![Value::Isize(-500)], Type::Isize)?;
      assert!(matches!(resI32, Value::I32(42)));
      assert!(matches!(resI64, Value::I64(100000)));
      assert!(matches!(resIsize, Value::Isize(500)));

      Ok(())
    }.expect("Signed integers test failed");
  }

  /// Checks all unsigned integer types (U8, U16, U32, U64, Usize).
  #[test]
  fn unsignedIntegers() -> ()
  {
    ffi!{
      let libc: Library = Library::load("libc.so.6")?;

      // U8, U16, U32
      let resU8: Value = libc.call("strnlen", vec![Value::CString(b"a".to_vec()), Value::U8(10)], Type::U8)?;
      let resU16: Value = libc.call("strnlen", vec![Value::CString(b"ab".to_vec()), Value::U16(10)], Type::U16)?;
      let resU32: Value = libc.call("strnlen", vec![Value::CString(b"abc".to_vec()), Value::U32(10)], Type::U32)?;
      assert!(matches!(resU8, Value::U8(1)));
      assert!(matches!(resU16, Value::U16(2)));
      assert!(matches!(resU32, Value::U32(3)));

      // U64 & Usize
      let resU64: Value = libc.call("strnlen", vec![Value::CString(b"abcd".to_vec()), Value::U64(10)], Type::U64)?;
      let resUsize: Value = libc.call("strnlen", vec![Value::CString(b"abcde".to_vec()), Value::Usize(10)], Type::Usize)?;
      assert!(matches!(resU64, Value::U64(4)));
      assert!(matches!(resUsize, Value::Usize(5)));

      Ok(())
    }.expect("Unsigned integers test failed");
  }

  /// Checks passing floating point numbers (F32, F64).
  #[test]
  fn float() -> ()
  {
    let resultF32: Value = ffi!{
      let libm: Library = Library::load("libm.so.6")?;
      let args: Vec<Value> = vec![Value::F32(16.0)];
      Ok(libm.call("sqrtf", args, Type::F32)?)
    }.expect("FFI F32 call failed");

    if let Value::F32(val) = resultF32 {
      assert!((val - 4.0).abs() < f32::EPSILON);
    } else {
      panic!("Expected Value::F32");
    }

    let resultF64: Value = ffi!{
      let libm: Library = Library::load("libm.so.6")?;
      let args: Vec<Value> = vec![Value::F64(2.0), Value::F64(3.0)];
      Ok(libm.call("pow", args, Type::F64)?)
    }.expect("FFI F64 call failed");

    if let Value::F64(val) = resultF64 {
      assert!((val - 8.0).abs() < f64::EPSILON);
    } else {
      panic!("Expected Value::F64");
    }
  }

  // ===============================================================================================

  /// Checks passing Bool and Usize types.
  #[test]
  fn bool() -> ()
  {
    let resultBool: Value = ffi!{
      let libc: Library = Library::load("libc.so.6")?;
      let args: Vec<Value> = vec![Value::Bool(true)];
      Ok(libc.call("isalpha", args, Type::Bool)?)
    }.expect("FFI Bool call failed");

    if let Value::Bool(val) = resultBool {
      assert!(!val);
    } else {
      panic!("Expected Value::Bool");
    }
  }

  // ===============================================================================================

  /// Checks pointer type handling: passing a valid pointer and receiving NULL for a missing variable.
  #[test]
  fn pointer() -> ()
  {
    let result: Value = ffi!{
    let libc: Library = Library::load("libc.so.6")?;
    let args: Vec<Value> = vec![Value::CString(b"noSuchVar".to_vec())];
    Ok(libc.call("getenv", args, Type::Pointer)?)
  }.expect("FFI pointer call failed");

    match result {
      Value::Pointer(addr) => assert_eq!(addr, 0),
      _ => panic!("Expected Value::Pointer"),
    }
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
    let len: Value = ffi!{
    let libc: Library = Library::load("libc.so.6")?;
    let source: Vec<Value> = vec![Value::CString(b"hello".to_vec())];
    let ptr: Value = libc.call("strdup", source, Type::Pointer)?;
    let Value::Pointer(addr) = ptr else { return Err(FFIError::Other("expected pointer".into())) };
    assert_ne!(addr, 0);

    let result: Value = libc.call("strlen", vec![Value::Pointer(addr)], Type::Usize)?;
    libc.call("free", vec![Value::Pointer(addr)], Type::None)?;
    Ok(result)
  }.expect("pointer roundtrip failed");

    assert!(matches!(len, Value::Usize(5)));
  }

  // ===============================================================================================

  /// Checks passing CString to a C function expecting a \0-terminated string (strlen).
  #[test]
  fn cString() -> ()
  {
    let result: Value = ffi!{
      let libc: Library = Library::load("libc.so.6")?;
      let args: Vec<Value> = vec![Value::CString(b"hello".to_vec())];
      Ok(libc.call("strlen", args, Type::Usize)?)
    }.expect("FFI CString call failed");

    if let Value::Usize(len) = result {
      assert_eq!(len, 5);
    } else {
      panic!("Expected Value::Usize");
    }
  }

  /// Checks passing String which automatically expands to two C-ABI arguments (ptr + len)
  /// for functions accepting buffer pointer and max length (strnlen).
  #[test]
  fn string() -> ()
  {
    let result: Value = ffi!{
      let libc: Library = Library::load("libc.so.6")?;
      let args: Vec<Value> = vec![Value::String(b"hello world".to_vec())];
      Ok(libc.call("strnlen", args, Type::Usize)?)
    }.expect("FFI String call failed");

    if let Value::Usize(len) = result {
      assert_eq!(len, 11);
    } else {
      panic!("Expected Value::Usize");
    }
  }

  /// Checks passing RawString as a single raw byte pointer (atoi).
  #[test]
  fn rawString() -> ()
  {
    let result: Value = ffi!{
      let libc: Library = Library::load("libc.so.6")?;
      let args: Vec<Value> = vec![Value::RawString(b"12345\0".to_vec())];
      Ok(libc.call("atoi", args, Type::I32)?)
    }.expect("FFI RawString call failed");

    if let Value::I32(val) = result {
      assert_eq!(val, 12345);
    } else {
      panic!("Expected Value::I32");
    }
  }

  // ===============================================================================================
}

// =================================================================================================