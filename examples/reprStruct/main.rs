#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::{TimeLibPath, TimeSymbolName};
#[cfg(windows)]
use crate::platform::FileTimeUnixEpoch;
// =================================================================================================
use bytemuck::{Pod, Zeroable};
use chillffi::ffi;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
// =================================================================================================

/// The wall clock's out-parameter as `#[repr(C)]` + `Pod`. Without `repr(C)`
/// fields may be reordered; `Pod` catches padding at compile time.
///
/// Unix: `timespec { time_t tv_sec; long tv_nsec; }`.
#[cfg(unix)]
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct Clock { secs: i64, nanos: i64 }

/// Windows: `FILETIME` — one u64 of 100ns ticks since 1601-01-01.
#[cfg(windows)]
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct Clock { ticks: u64 }

impl Clock
{
  const fn split(self) -> (i64, i64)
  {
    #[cfg(unix)]
    { (self.secs, self.nanos) }

    #[cfg(windows)]
    {
      let sinceEpoch: u64 = self.ticks.saturating_sub(FileTimeUnixEpoch);
      ((sinceEpoch / 10_000_000) as i64, ((sinceEpoch % 10_000_000) * 100) as i64)
    }
  }
}

/// Plain 16-byte POD used only for the write/read roundtrip below — not
/// tied to any OS layout.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug, PartialEq)]
struct Sample { a: i64, b: i64 }

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
    let clockLib: Library = scope.load(TimeLibPath)?;
    let mem: AllocatedMemory = scope.alloc(size_of::<Clock>())?;

    #[cfg(unix)]
    clockLib.call(TimeSymbolName).arg::<i32>(0 /* CLOCK_REALTIME */).arg(mem.asPointer()).void()?;
    #[cfg(windows)]
    clockLib.call(TimeSymbolName).arg(mem.asPointer()).void()?;

    let bytes: Vec<u8> = mem.read()?;

    #[cfg(unix)]
    let parsed: (i64, i64) = (
      i64::from_ne_bytes(bytes[0..8].try_into().unwrap()),
      i64::from_ne_bytes(bytes[8..16].try_into().unwrap())
    );

    #[cfg(windows)]
    let parsed: (i64, i64) = {
      let ticks: u64 = u64::from_ne_bytes(bytes[0..8].try_into().unwrap());
      let sinceEpoch: u64 = ticks.saturating_sub(FileTimeUnixEpoch);
      ((sinceEpoch / 10_000_000) as i64, ((sinceEpoch % 10_000_000) * 100) as i64)
    };

    Ok(parsed)
  }).expect("manual parsing failed");

  println!("manual:  realtime = {secs}.{nanos:09}");
}

/// Same thing via `readStruct`.
fn readStructRemovesManualParsing() -> ()
{
  let clock: Clock = ffi!(|scope| {
    let clockLib: Library = scope.load(TimeLibPath)?;
    let mem: AllocatedMemory = scope.alloc(size_of::<Clock>())?;

    #[cfg(unix)]
    clockLib.call(TimeSymbolName).arg::<i32>(0 /* CLOCK_REALTIME */).arg(mem.asPointer()).void()?;
    #[cfg(windows)]
    clockLib.call(TimeSymbolName).arg(mem.asPointer()).void()?;

    mem.readStruct::<Clock>()
  }).expect("readStruct failed");

  let (secs, nanos): (i64, i64) = clock.split();
  println!("typed:   realtime = {secs}.{nanos:09}");
}

/// `writeStruct` — write a Rust value into the buffer.
fn writeThenReadStructRoundtrip() -> ()
{
  let (original, readBack): (Sample, Sample) = ffi!(|scope| {
    let mem: AllocatedMemory = scope.alloc(size_of::<Sample>())?;

    let original: Sample = Sample { a: 1_700_000_000, b: 123_456_789 };
    mem.writeStruct(&original)?;

    let readBack: Sample = mem.readStruct::<Sample>()?;
    Ok((original, readBack))
  }).expect("writeStruct/readStruct roundtrip failed");

  assert_eq!(original, readBack, "readStruct should return exactly what writeStruct wrote");
  println!("ok: writeStruct/readStruct roundtrip -> {readBack:?}");
}

// =================================================================================================
