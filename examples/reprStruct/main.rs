#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::LibcPath;
// =================================================================================================
use bytemuck::{Pod, Zeroable};
use chillffi::ffi;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
// =================================================================================================

/// `timespec` as `#[repr(C)]` + `Pod`. Without `repr(C)` fields may be reordered;
/// `Pod` catches padding at compile time.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug, PartialEq)]
struct Timespec { secs: i64, nanos: i64 }

/// `readStruct` / `writeStruct` — typed access to a buffer via `#[repr(C)]`.
fn main() -> ()
{
  manualByteParsing();
  readStructRemovesManualParsing();
  writeThenReadStructRoundtrip();
}

// =================================================================================================

/// Manual parse with slices and `from_ne_bytes`.
fn manualByteParsing() -> ()
{
  let (secs, nanos): (i64, i64) = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    let mem: AllocatedMemory = scope.alloc(16)?;

    libc.call("clock_gettime").arg::<i32>(0 /* CLOCK_REALTIME */).arg(mem.asPointer()).void()?;

    let bytes: Vec<u8> = mem.read()?;
    let secs: i64 = i64::from_ne_bytes(bytes[0..8].try_into().unwrap());
    let nanos: i64 = i64::from_ne_bytes(bytes[8..16].try_into().unwrap());
    Ok((secs, nanos))
  }).expect("manual parsing failed");

  println!("manual:  clock_gettime = {secs}.{nanos:09}");
}

/// Same thing via `readStruct`.
fn readStructRemovesManualParsing() -> ()
{
  let ts: Timespec = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    let mem: AllocatedMemory = scope.alloc(size_of::<Timespec>())?;

    libc.call("clock_gettime").arg::<i32>(0 /* CLOCK_REALTIME */).arg(mem.asPointer()).void()?;

    mem.readStruct::<Timespec>()
  }).expect("readStruct failed");

  println!("typed:   clock_gettime = {}.{:09}", ts.secs, ts.nanos);
}

/// `writeStruct` — write a Rust value into the buffer.
fn writeThenReadStructRoundtrip() -> ()
{
  let (original, readBack): (Timespec, Timespec) = ffi!(|scope| {
    let mem: AllocatedMemory = scope.alloc(size_of::<Timespec>())?;

    let original: Timespec = Timespec { secs: 1_700_000_000, nanos: 123_456_789 };
    mem.writeStruct(&original)?;

    let readBack: Timespec = mem.readStruct::<Timespec>()?;
    Ok((original, readBack))
  }).expect("writeStruct/readStruct roundtrip failed");

  assert_eq!(original, readBack, "readStruct should return exactly what writeStruct wrote");
  println!("ok: writeStruct/readStruct roundtrip -> {readBack:?}");
}

// =================================================================================================
