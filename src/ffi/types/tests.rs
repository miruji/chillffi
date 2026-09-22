use crate::ffi;
use std::ffi::CString;
use crate::ffi::types::primitive::{Arg, Pointer, StructValue};
use crate::ffi::types::{Type, Value};
use crate::platform::{platformExt, LibcPath, LibmPath, StrdupSymbolName};
// =================================================================================================

/// Checks all signed integer types 
///
/// [`Value::I8`], [`Value::I16`], [`Value::I32`], [`Value::I64`], [`Value::Isize`]
#[test]
fn signedIntegers() -> ()
{
  ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;

    // abs() takes and returns `int` everywhere. Declaring the FFI call
    // itself as i8/i16 relies on the ABI sign-extending into the full
    // register — true on SysV x86-64 and apparently Windows ARM64, not on
    // Windows x64 — so the call always uses i32, and narrowing to i8/i16
    // happens in Rust afterward (no ABI involved, always correct).
    let resI8: i8 = libc.call("abs").arg::<i32>(-5).result::<i32>()? as i8;
    assert!(matches!(resI8, 5));

    let resI16: i16 = libc.call("abs").arg::<i32>(-15).result::<i32>()? as i16;
    assert!(matches!(resI16, 15));

    let resI32: i32 = libc.call("abs").arg::<i32>(-42).result()?;
    assert!(matches!(resI32, 42));

    // labs() takes `long`: 64-bit on Unix (LP64), only 32-bit on Windows
    // (LLP64). The call has to match whichever width the platform's C ABI
    // actually gives `long`, not size_of::<isize>().
    #[cfg(unix)]
    let resI64: i64 = libc.call("labs").arg::<i64>(-100000).result()?;
    #[cfg(windows)]
    let resI64: i64 = libc.call("labs").arg::<i32>(-100000).result::<i32>()? as i64;
    assert!(matches!(resI64, 100000));

    #[cfg(unix)]
    let resIsize: isize = libc.call("labs").arg::<isize>(-500).result()?;
    #[cfg(windows)]
    let resIsize: isize = libc.call("labs").arg::<i32>(-500).result::<i32>()? as isize;
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
    let libc: Library = scope.load(LibcPath)?;
    
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
    let libm: Library = scope.load(LibmPath)?;
    libm.call("sqrtf").arg::<f32>(16.0).result()
  }).expect("FFI F32 call failed");

  assert!((resultF32 - 4.0).abs() < f32::EPSILON);

  let resultF64: f64 = ffi!(|scope| {
    let libm: Library = scope.load(LibmPath)?;
    libm.call("pow")
      .arg::<f64>(2.0)
      .arg::<f64>(3.0)
      .result()
  }).expect("FFI F64 call failed");

  assert!((resultF64 - 8.0).abs() < f64::EPSILON);
}

// =================================================================================================

/// Checks passing [`Value::Bool`].
#[test]
fn bool() -> ()
{
  let result: bool = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    libc.call("isalpha").arg(true).result()
  }).expect("FFI Bool call failed");

  assert!(!result);
}

// =================================================================================================

/// Checks pointer type handling: passing a valid pointer 
/// and receiving NULL for a missing variable.
#[test]
fn pointer() -> ()
{
  let result: Pointer = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    libc.call("getenv")
      .arg(c"noSuchVar")
      .result()
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
    let libc: Library = scope.load(LibcPath)?;
    let ptr: Pointer = 
      libc.call(StrdupSymbolName)
        .arg(c"hello")
        .result()?;
    assert_ne!(ptr, Pointer(0));

    let result: usize = libc.call("strlen").arg(ptr).result()?;
    libc.call("free").arg(ptr).void()?;
    Ok(result)
  }).expect("pointer roundtrip failed");

  assert!(matches!(len, 5));
}

// =================================================================================================

/// Checks string conversion bridges and `TryFrom` validation.
#[test]
fn stringBridges() -> ()
{
  let valString: Value = String::from("hello").into();
  assert!(matches!(valString, Value::String(_)));

  let valStr: Value = "hello".into();
  assert_eq!(valStr, Value::String(b"hello".to_vec()));

  let valCString: Value = CString::new("hello").unwrap().into();
  assert_eq!(valCString, c"hello".into());

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
    let libc: Library = scope.load(LibcPath)?;
    libc.call("strlen")
      .arg(c"hello")
      .result()
  }).expect("FFI CString call failed");

  assert_eq!(result, 5);
}

/// Checks passing String which automatically expands to two C-ABI arguments (ptr + len)
/// for functions accepting buffer pointer and max length (strnlen).
#[test]
fn string() -> ()
{
  let result: usize = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    libc.call("strnlen")
      .arg("hello world")
      .result()
  }).expect("FFI String call failed");

  assert_eq!(result, 11);
}

/// Checks passing RawString as a single raw byte pointer (atoi).
#[test]
fn rawString() -> ()
{
  let result: i32 = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    libc.call("atoi")
      .arg(b"12345\0".to_vec())
      .result()
  }).expect("FFI RawString call failed");

  assert_eq!(result, 12345);
}

// =================================================================================================

/// Checks by-value struct as a call argument and as a return value.
///
/// Uses the `byValueStruct` example library (`point_sum` / `point_translate`).
/// Small `struct Point { i32, i32 }` is typically passed in registers.
#[test]
fn byValueStructArgAndResult() -> ()
{
  // Argument side: StructValue is serialized into a buffer and handed to
  // libffi as a single by-value CIF argument.
  let sum: i32 = ffi!(|scope| {
    scope.addSearchPath("examples/byValueStruct");
    let lib: Library = scope.load(platformExt!("libbyvalue"))?;

    let point: StructValue = StructValue::new([
      Arg::from(10i32),
      Arg::from(32i32),
    ]);

    lib.call("pointSum").arg(point).result()
  }).expect("by-value arg failed");

  assert_eq!(sum, 42);

  // Result side: `.resultStruct` needs an explicit Type layout because the
  // return buffer is untyped until decoded by readStructAt.
  let (x, y): (i32, i32) = ffi!(|scope| {
    scope.addSearchPath("examples/byValueStruct");
    let lib: Library = scope.load(platformExt!("libbyvalue"))?;

    let point: StructValue = StructValue::new([
      Arg::from(1i32),
      Arg::from(2i32),
    ]);

    let out: StructValue = lib
      .call("pointTranslate")
      .arg(point)
      .arg(10i32)
      .arg(20i32)
      .resultStruct(&[Type::I32, Type::I32])?;

    Ok((out.get(0)?, out.get(1)?))
  }).expect("by-value result failed");

  assert_eq!((x, y), (11, 22));
}

/// Checks a by-value struct larger than 16 bytes.
///
/// On x86_64 SysV such aggregates are passed via memory / the stack rather
/// than registers; libffi selects the correct ABI path from the CIF type.
#[test]
fn byValueLargeStruct() -> ()
{
  let sum: f64 = ffi!(|scope| {
    scope.addSearchPath("examples/byValueStruct");
    let lib: Library = scope.load(platformExt!("libbyvalue"))?;

    // struct Big { f64 a; f64 b; f64 c; i32 tag; } — padding after tag is
    // computed by structLayout, not by the caller.
    let big: StructValue = StructValue::new([
      Arg::from(1.5f64),
      Arg::from(2.5f64),
      Arg::from(3.0f64),
      Arg::from(7i32),
    ]);

    lib.call("bigSum").arg(big).result()
  }).expect("large by-value arg failed");

  assert!((sum - 14.0).abs() < 1e-9);
}

/// Checks nested by-value structs on both the argument and result sides.
///
/// `struct Nested { struct Point origin; float scale; }` — the inner Point
/// is itself a `StructValue` field, and comes back through `getStruct`.
#[test]
fn byValueNestedStruct() -> ()
{
  let (ox, oy, sc): (i32, i32, f32) = ffi!(|scope| {
    scope.addSearchPath("examples/byValueStruct");
    let lib: Library = scope.load(platformExt!("libbyvalue"))?;

    // Nested { Point { i32, i32 }, f32 }.
    let nested: StructValue = StructValue::new([
      Arg::from(StructValue::new([Arg::from(5i32), Arg::from(6i32)])),
      Arg::from(1.5f32),
    ]);

    let out: StructValue = lib
      .call("nestedDouble")
      .arg(nested)
      .resultStruct(&[
        Type::structure([Type::I32, Type::I32]),
        Type::F32,
      ])?;

    // Field 0 is the nested Point — extract with getStruct, then scalars.
    let origin: StructValue = out.getStruct(0)?;
    Ok((origin.get(0)?, origin.get(1)?, out.get(1)?))
  }).expect("nested by-value failed");

  assert_eq!((ox, oy), (10, 12));
  assert!((sc - 3.0).abs() < 1e-5);
}

// =================================================================================================
