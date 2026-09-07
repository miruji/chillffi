use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::errors::FFIError;
use chillffi::ffi;
use bytemuck::{Pod, Zeroable};
// =================================================================================================

/// struct timespec { time_t tv_sec; long tv_nsec; } — 16 bytes on x86_64 Linux.
/// #[repr(C)] is mandatory — otherwise Rust is free to reorder the fields.
/// derive(Pod) checks at compile time that there is no padding: if there were,
/// for example, i32+i64 without an explicit _pad, bytemuck would refuse to compile,
/// rather than silently letting you read garbage from the gap.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct Timespec { secs: i64, nanos: i64 }

// =================================================================================================

/// BEFORE: what is currently in examples/clock.rs — manual byte parsing.
fn clockGettimeManual() -> Result<(i64, i64), FFIError>
{
  ffi!(|scope| {
    let libc: Library = scope.load("libc.so.6")?;
    let mem: AllocatedMemory = scope.alloc(16)?;

    libc.call("clock_gettime")
      .arg::<i32>(0 as i32) // CLOCK_REALTIME
      .arg(mem.asPointer())
      .void()?;

    let bytes: Vec<u8> = mem.read()?;

    let secs: i64 = i64::from_ne_bytes(bytes[0..8].try_into().unwrap());
    let nanos: i64 = i64::from_ne_bytes(bytes[8..16].try_into().unwrap());
    Ok((secs, nanos))
  })
}

/// AFTER: readStruct removes all manual parsing — the size and field layout
/// are checked by the type rather than by manually calculated slices.
fn clockGettimeTyped() -> Result<Timespec, FFIError>
{
  ffi!(|scope| {
    let libc: Library = scope.load("libc.so.6")?;
    let mem: AllocatedMemory = scope.alloc(std::mem::size_of::<Timespec>())?;

    libc.call("clock_gettime")
      .arg::<i32>(0 as i32) // CLOCK_REALTIME
      .arg(mem.asPointer())
      .void()?;

    mem.readStruct::<Timespec>()
  })
}

// =================================================================================================

/// allocAligned: buffer for SSE (__m128 requires 16-byte alignment).
/// A regular scope.alloc() malloc buffer provides sufficient alignment for most
/// types, but does not guarantee specifically 16/32/64 — this is exactly the case
/// where a bare alloc() is not sufficient.
fn simdAlignedBuffer() -> Result<usize, FFIError>
{
  ffi!(|scope| {
    let mem: AllocatedMemory = scope.allocAligned(64, 16)?; // 64 bytes, aligned to 16
    let addr: usize = usize::from(mem.asPointer());

    // The very fact that this is an assert rather than "however it happens to work"
    // is the difference between allocAligned and a regular alloc().
    assert_eq!(addr % 16, 0, "posix_memalign was required to return an aligned address");

    Ok(addr)
  })
}

// =================================================================================================

fn main()
{
  let (secs, nanos): (i64, i64) = clockGettimeManual().expect("manual failed");
  println!("manual:  {secs}.{nanos:09}");

  let ts: Timespec = clockGettimeTyped().expect("typed failed");
  println!("typed:   {}.{:09}", ts.secs, ts.nanos);

  let addr: usize = simdAlignedBuffer().expect("aligned alloc failed");
  println!("aligned buffer at 0x{addr:X}, % 16 == 0: verified");
}

// =================================================================================================
