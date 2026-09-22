#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::platformExt;
// =================================================================================================
use chillffi::ffi;
use chillffi::ffi::types::primitive::{Arg, StructValue};
use chillffi::ffi::types::Type;
// =================================================================================================

/// By-value structures: pass and return C structs without an out-parameter pointer.
///
/// Covers register-sized structs, large structs (>16 bytes on x86_64 SysV),
/// and nested structs — all going through libffi's ABI classification.
fn main() -> ()
{
  pointSum();
  pointTranslate();
  bigStruct();
  nestedStruct();
}

// =================================================================================================

/// Small struct passed by value as a call argument (typically in registers).
///
/// `struct Point { int32_t x; int32_t y; }` → `pointSum(Point)`.
fn pointSum() -> ()
{
  let sum: i32 = ffi!(|scope| {
    scope.addSearchPath("examples/byValueStruct");
    let lib: Library = scope.load(platformExt!("libbyvalue"))?;

    // struct Point { i32 x; i32 y; } — field types inferred from the Args.
    let point: StructValue = StructValue::new([
      Arg::from(10i32),
      Arg::from(32i32),
    ]);

    lib.call("pointSum").arg(point).result()
  }).expect("pointSum failed");

  assert_eq!(sum, 42);
  println!("ok: pointSum(by-value) -> {sum}");
}

/// Struct returned by value — decoded through `.resultStruct(&[Type::...])`.
///
/// `pointTranslate(Point, dx, dy) -> Point`.
fn pointTranslate() -> ()
{
  let (x, y): (i32, i32) = ffi!(|scope| {
    scope.addSearchPath("examples/byValueStruct");
    let lib: Library = scope.load(platformExt!("libbyvalue"))?;

    let point: StructValue = StructValue::new([
      Arg::from(1i32),
      Arg::from(2i32),
    ]);

    // Layout must match the C definition of struct Point.
    let out: StructValue = lib
      .call("pointTranslate")
      .arg(point)
      .arg(10i32)
      .arg(20i32)
      .resultStruct(&[Type::I32, Type::I32])?;

    Ok((out.get(0)?, out.get(1)?))
  }).expect("pointTranslate failed");

  assert_eq!((x, y), (11, 22));
  println!("ok: pointTranslate(by-value) -> ({x}, {y})");
}

/// Struct larger than 16 bytes — on x86_64 SysV this goes via memory/stack
/// rather than registers; libffi handles the ABI classification.
///
/// `struct Big { double a, b, c; int32_t tag; }` → `bigSum` / `bigScale`.
fn bigStruct() -> ()
{
  let sum: f64 = ffi!(|scope| {
    scope.addSearchPath("examples/byValueStruct");
    let lib: Library = scope.load(platformExt!("libbyvalue"))?;

    // struct Big { f64 a; f64 b; f64 c; i32 tag; } — padding after tag is
    // filled by structLayout / libffi, not by the caller.
    let big: StructValue = StructValue::new([
      Arg::from(1.5f64),
      Arg::from(2.5f64),
      Arg::from(3.0f64),
      Arg::from(7i32),
    ]);

    lib.call("bigSum").arg(big).result()
  }).expect("bigSum failed");

  assert!((sum - 14.0).abs() < 1e-9, "bigSum = {sum}");
  println!("ok: bigSum(by-value >16B) -> {sum}");

  let (a, tag): (f64, i32) = ffi!(|scope| {
    scope.addSearchPath("examples/byValueStruct");
    let lib: Library = scope.load(platformExt!("libbyvalue"))?;

    let big: StructValue = StructValue::new([
      Arg::from(1.0f64),
      Arg::from(2.0f64),
      Arg::from(3.0f64),
      Arg::from(4i32),
    ]);

    let out: StructValue = lib
      .call("bigScale")
      .arg(big)
      .arg(2.0f64)
      .resultStruct(&[Type::F64, Type::F64, Type::F64, Type::I32])?;

    Ok((out.get(0)?, out.get(3)?))
  }).expect("bigScale failed");

  assert!((a - 2.0).abs() < 1e-9);
  assert_eq!(tag, 4);
  println!("ok: bigScale(by-value return) -> a={a}, tag={tag}");
}

/// Nested struct by value: outer holds an inner `Point` as a field.
///
/// `struct Nested { Point origin; float scale; }` → `nestedScale` / `nestedDouble`.
fn nestedStruct() -> ()
{
  let scale: f32 = ffi!(|scope| {
    scope.addSearchPath("examples/byValueStruct");
    let lib: Library = scope.load(platformExt!("libbyvalue"))?;

    // Nested { Point { i32, i32 }, f32 } — inner StructValue becomes one field.
    let nested: StructValue = StructValue::new([
      Arg::from(StructValue::new([Arg::from(3i32), Arg::from(4i32)])),
      Arg::from(2.5f32),
    ]);

    lib.call("nestedScale").arg(nested).result()
  }).expect("nestedScale failed");

  assert!((scale - 17.5).abs() < 1e-5, "nestedScale = {scale}");
  println!("ok: nestedScale(by-value nested) -> {scale}");

  let (ox, oy, sc): (i32, i32, f32) = ffi!(|scope| {
    scope.addSearchPath("examples/byValueStruct");
    let lib: Library = scope.load(platformExt!("libbyvalue"))?;

    let nested: StructValue = StructValue::new([
      Arg::from(StructValue::new([Arg::from(5i32), Arg::from(6i32)])),
      Arg::from(1.5f32),
    ]);

    // Return layout mirrors the C definition, including the nested Point.
    let out: StructValue = lib
      .call("nestedDouble")
      .arg(nested)
      .resultStruct(&[
        Type::structure([Type::I32, Type::I32]),
        Type::F32,
      ])?;

    // Field 0 is itself a struct — pull it out with getStruct, then scalars.
    let origin: StructValue = out.getStruct(0)?;
    Ok((origin.get(0)?, origin.get(1)?, out.get(1)?))
  }).expect("nestedDouble failed");

  assert_eq!((ox, oy), (10, 12));
  assert!((sc - 3.0).abs() < 1e-5);
  println!("ok: nestedDouble(by-value nested return) -> origin=({ox},{oy}) scale={sc}");
}

// =================================================================================================
