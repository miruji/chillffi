#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::{TimeLibPath, TimeSymbolName};
use crate::platform::platformExt;
#[cfg(windows)]
use crate::platform::FileTimeUnixEpoch;
// =================================================================================================
use chillffi::ffi;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::scope::Scope;
use chillffi::ffi::types::primitive::{Arg, DynamicList, Pointer};
use chillffi::ffi::types::Type;
// =================================================================================================

/// Dynamic structs: layout is a runtime `Vec<Type>`, no Rust type required.
fn main() -> ()
{
  readDynamicStruct();
  structAsCallParameter();
  structAsCallResult();
  nestedStructShapeIsSized();
}

// =================================================================================================

/// Read the OS wall clock's out-parameter via a runtime shape.
fn readDynamicStruct() -> ()
{
  let (secs, nanos): (i64, i64) = ffi!(|scope| {
    let clockLib: Library = scope.load(TimeLibPath)?;
    let mem: AllocatedMemory = scope.alloc(16)?;

    #[cfg(unix)]
    clockLib.call(TimeSymbolName).arg::<i32>(0).arg(mem.asPointer()).void()?;
    #[cfg(windows)]
    clockLib.call(TimeSymbolName).arg(mem.asPointer()).void()?;

    #[cfg(unix)]
    let parsed: (i64, i64) = {
      // struct timespec { time_t tv_sec; long tv_nsec; }
      let fields: DynamicList = Scope::readDynamicStruct(mem.address(), &[Type::I64, Type::I64])?;
      (fields.get(0)?, fields.get(1)?)
    };

    #[cfg(windows)]
    let parsed: (i64, i64) = {
      // FILETIME { u64 ticks }
      let fields: DynamicList = Scope::readDynamicStruct(mem.address(), &[Type::U64])?;
      let ticks: u64 = fields.get(0)?;
      let sinceEpoch: u64 = ticks.saturating_sub(FileTimeUnixEpoch);
      ((sinceEpoch / 10_000_000) as i64, ((sinceEpoch % 10_000_000) * 100) as i64)
    };

    Ok(parsed)
  }).expect("readDynamicStruct failed");

  println!("ok: readDynamicStruct -> realtime = {secs}.{nanos:09}");
}

/// ```c
/// struct Data { int size; int *values; };
/// void process(struct Data *data);
/// ```
/// Runtime layout, `allocStruct` + `writeDynamicStruct`.
fn structAsCallParameter() -> ()
{
  let dataShape: Vec<Type> = vec![Type::I32, Type::Pointer];
  let values: Vec<i32> = vec![10, 20, 30];

  let sum: i32 = ffi!(|scope| {
    scope.addSearchPath("examples/dynamicStruct");
    let lib: Library = scope.load(platformExt!("libparameter"))?;

    let valuesMem: AllocatedMemory = scope.alloc(values.len() * size_of::<i32>())?;
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_ne_bytes()).collect();
    Scope::writeMemory(valuesMem.address(), bytes)?;

    let dataMem: AllocatedMemory = scope.allocStruct(&dataShape)?;
    Scope::writeDynamicStruct(dataMem.address(), &dataShape, vec![
      Arg::from(values.len() as i32),
      Arg::from(valuesMem.asPointer()),
    ])?;

    lib.call("process").arg(dataMem.asPointer()).void()?;

    // dataMem/valuesMem free themselves (Drop) when this block ends — process()
    // only read from `data`, it never took ownership of the allocation.
    lib.call("getSum").result()
  }).expect("dynamic struct parameter failed");

  assert_eq!(sum, 60);
  println!("ok: dynamic struct as a call parameter -> sum = {sum}");
}

/// ```c
/// struct Data { int size; int *values; };
/// struct Data *process(void);
/// ```
/// Read the returned struct; the array is read separately using the `size` field.
fn structAsCallResult() -> ()
{
  let values: Vec<i32> = ffi!(|scope| {
    scope.addSearchPath("examples/dynamicStruct");
    let lib: Library = scope.load(platformExt!("libresult"))?;

    let dataPtr: Pointer = lib.call("process").result()?;

    let fields: DynamicList = Scope::readDynamicStruct(dataPtr, &[Type::I32, Type::Pointer])?;
    let size: i32 = fields.get(0)?;
    let valuesPtr: Pointer = fields.get(1)?;

    let bytes: Vec<u8> = Scope::readMemory(valuesPtr, size as usize * 4)?;
    let values: Vec<i32> = bytes.as_chunks::<4>().0
      .iter()
      .map(|b| i32::from_ne_bytes(*b))
      .collect();

    // process() malloc'd both the struct and its values array — free both
    // through the C side's own freeData(), since only the C side knows the
    // true allocation shape (here, that `values` was a separate malloc).
    lib.call("freeData").arg(dataPtr).void()?;

    Ok(values)
  }).expect("dynamic struct result failed");

  assert_eq!(values, vec![10, 20, 30]);
  println!("ok: dynamic struct as a call result -> {values:?}");
}

/// Nested `Type::Struct` is supported by the layout engine (`allocStruct`
/// computes size with padding). Reading a nested field via the public API
/// is not possible yet — `get::<T>` only accepts scalars.
fn nestedStructShapeIsSized() -> ()
{
  let shape: Vec<Type> = vec![
    Type::Struct(Box::new([Type::U64, Type::U8, Type::F64])),
    Type::I8,
    Type::I64,
  ];

  let size: usize = ffi!(|scope| {
    let mem: AllocatedMemory = scope.allocStruct(&shape)?;
    Ok(mem.length())
  }).expect("allocStruct with a nested Type::Struct failed");

  assert!(size > 0);
  println!("ok: nested Type::Struct shape sized to {size} bytes (extraction not yet public)");
}

// =================================================================================================
