#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::LibcPath;
// =================================================================================================
use chillffi::callback;
use chillffi::ffi;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::types::primitive::Callback;
use chillffi::ffi::types::primitive::Pointer;
use std::cmp::Ordering;
// =================================================================================================

/// `callback!` — Rust closure as a C function pointer.
/// Capture is automatic; captured values must be `Copy + Send`.
fn main() -> ()
{
  noCaptures();
  multipleCapturedVariables();
}

// =================================================================================================

/// Comparator with no captures.
fn noCaptures() -> ()
{
  let sorted: Vec<i32> = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    let mem: AllocatedMemory = scope.alloc(5 * 4)?;

    let data: [i32; 5] = [3, 1, 4, 1, 5];
    let raw: &[u8] = unsafe{ std::slice::from_raw_parts(data.as_ptr() as *const u8, 20) };
    mem.write(raw)?;

    // Direct dereferencing is correct: the closure runs inside the clone
    // (where the data resides), not in the parent process.
    let compar: Callback = callback!(scope, |a: Pointer, b: Pointer| -> i32 {
      let av: i32 = unsafe{ *(a.0 as *const i32) };
      let bv: i32 = unsafe{ *(b.0 as *const i32) };
      av.cmp(&bv) as i32
    });

    libc.call("qsort")
      .arg(mem.asPointer())
      .arg::<usize>(5)
      .arg::<usize>(4)
      .arg(compar)
      .void()?;

    let bytes: Vec<u8> = mem.read()?;
    Ok(bytes.as_chunks::<4>().0
      .iter()
      .map(|b| i32::from_ne_bytes(*b))
      .collect())
  }).expect("qsort failed");

  assert_eq!(sorted, vec![1, 1, 3, 4, 5]);
  println!("ok: no-capture comparator -> {sorted:?}");
}

/// Capture of two variables of different types (`i32` and `bool`).
fn multipleCapturedVariables() -> ()
{
  let bias: i32 = 3; // Shifts every value before comparing
  let descending: bool = true; // Flips the resulting order

  let sorted: Vec<i32> = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    let mem: AllocatedMemory = scope.alloc(5 * 4)?;

    let data: [i32; 5] = [3, 1, 4, 1, 5];
    let raw: &[u8] = unsafe{ std::slice::from_raw_parts(data.as_ptr() as *const u8, 20) };
    mem.write(raw)?;

    let compar: Callback = callback!(scope, |a: Pointer, b: Pointer| -> i32 {
      let av: i32 = unsafe{ *(a.0 as *const i32) };
      let bv: i32 = unsafe{ *(b.0 as *const i32) };

      let cmp: Ordering = if descending {
        (bv + bias).cmp(&(av + bias))
      } else {
        (av + bias).cmp(&(bv + bias))
      };
      cmp as i32
    });

    libc.call("qsort")
      .arg(mem.asPointer())
      .arg::<usize>(5)
      .arg::<usize>(4)
      .arg(compar)
      .void()?;

    let bytes: Vec<u8> = mem.read()?;
    Ok(bytes.as_chunks::<4>().0
      .iter()
      .map(|b| i32::from_ne_bytes(*b))
      .collect())
  }).expect("qsort failed");

  assert_eq!(sorted, vec![5, 4, 3, 1, 1]);
  println!("ok: multi-capture comparator (bias={bias}, descending={descending}) -> {sorted:?}");
}

// =================================================================================================
