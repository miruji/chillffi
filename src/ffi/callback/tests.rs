use crate::callback;
use crate::ffi;
use crate::ffi::allocatedMemory::AllocatedMemory;
use crate::ffi::callback::decode;
use crate::ffi::callback::sendable::Sendable;
use crate::ffi::callback::CallError;
use crate::ffi::callback::Envelope;
use crate::ffi::callback::ErasedCallable;
use crate::ffi::types::primitive::{Callback, Pointer};
use crate::platform::LibcPath;
// ===============================================================================================

/// Baseline: a closure with *no* captures at all round-trips through a
/// real registration, over real IPC, into a real clone, and back — the
/// degenerate (zero-sized) case of the bit-copy mechanism, exercised the
/// same way a real caller would use it.
#[test]
fn roundtrip() -> ()
{
  let sorted: Vec<i32> = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    let mem: AllocatedMemory = scope.alloc(4 * 4)?;

    let data: [i32; 4] = [3, 1, 4, 1];
    let raw: &[u8] = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, 16) };
    mem.write(raw)?;

    let compar: Callback = callback!(scope, |a: Pointer, b: Pointer| -> i32 {
      let av: i32 = unsafe { *(a.0 as *const i32) };
      let bv: i32 = unsafe { *(b.0 as *const i32) };
      av.cmp(&bv) as i32
    });

    libc.call("qsort")
      .arg(mem.asPointer())
      .arg::<usize>(4)
      .arg::<usize>(4)
      .arg(compar)
      .void()?;

    let bytes: Vec<u8> = mem.read()?;
    Ok(bytes.chunks_exact(4)
      .map(|b| i32::from_ne_bytes(b.try_into().unwrap()))
      .collect())
  }).expect("qsort failed");

  assert_eq!(sorted, vec![1, 1, 3, 4]);
}

/// A single value from the enclosing scope reaches the closure with no
/// explicit capture list — same idea as `examples/callback.rs` and
/// `examples/multiCapture.rs`, just as a plain assert instead of prints.
/// `threshold` is captured automatically, and the comparator runs inside a
/// real clone, so this proves it was actually serialized across, not just
/// visible by accident.
#[test]
fn externalCaptureReachesTheClone() -> ()
{
  let threshold: i32 = 3;

  let sorted: Vec<i32> = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    let mem: AllocatedMemory = scope.alloc(4 * 4)?;

    let data: [i32; 4] = [5, 1, 9, 2];
    let raw: &[u8] = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, 16) };
    mem.write(raw)?;

    // `threshold` isn't listed anywhere — just used directly below.
    let compar: Callback = callback!(scope, |a: Pointer, b: Pointer| -> i32 {
      let av: i32 = unsafe { *(a.0 as *const i32) } - threshold;
      let bv: i32 = unsafe { *(b.0 as *const i32) } - threshold;
      av.cmp(&bv) as i32
    });

    libc.call("qsort")
      .arg(mem.asPointer())
      .arg::<usize>(4)
      .arg::<usize>(4)
      .arg(compar)
      .void()?;

    let bytes: Vec<u8> = mem.read()?;
    Ok(bytes.chunks_exact(4)
      .map(|b| i32::from_ne_bytes(b.try_into().unwrap()))
      .collect())
  }).expect("qsort failed");

  assert_eq!(sorted, vec![1, 2, 5, 9]);
}

// ===============================================================================================

/// Requesting the wrong Args/Output must fail cleanly — checked by the
/// target function itself before any state is decoded.
///
/// Runs inside a real `ffi!` block, against a real clone, like the two
/// tests above — nothing here stands in for `Scope`. `callback!(@sendable, ...)`
/// is the exact same macro path as `callback!(scope, ...)` (the latter is
/// defined as this plus registration — see `callback!`'s definition), it
/// just stops one step short of shipping the bytes off, so the test can
/// corrupt them first and feed them to `decode` — the very same function
/// the clone calls when it receives a real registration.
#[test]
fn argsOutputMismatchIsCaught() -> ()
{
  ffi!(|scope| {
    // Proves registration really works inside this block before testing
    // the failure path below.
    let x: i32 = 1;
    let _sanity: Callback = callback!(scope, |y: i32| -> i32 { y + x });

    let sendable: Sendable = callback!(@sendable |y: i32| -> i32 { y + x });
    let bytes: Vec<u8> = sendable.encode().expect("encode");

    let (mut envelope, _): (Envelope, usize) =
      bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
    envelope.argsOutputTag = envelope.argsOutputTag.wrapping_add(1);
    let corrupted: Vec<u8> =
      bincode::serde::encode_to_vec(&envelope, bincode::config::standard()).unwrap();

    let result: Result<ErasedCallable, CallError> = decode(&corrupted);
    assert!( matches!(result, Err(CallError::ArgsOutputMismatch)) );

    Ok(())
  }).expect("ffi! failed");
}

/// A resolved-but-wrong site (simulated by hand-corrupting the tag) must be
/// caught by the target function itself, not silently produce garbage.
/// Same shape as the test above — real `ffi!`, real clone, real registration
/// path, `@sendable` only to get at the bytes to corrupt.
#[test]
fn siteTagMismatchIsCaught() -> ()
{
  ffi!(|scope| {
    let x: i32 = 1;
    let _sanity: Callback = callback!(scope, |y: i32| -> i32 { y + x });

    let sendable: Sendable = callback!(@sendable |y: i32| -> i32 { y + x });
    let bytes: Vec<u8> = sendable.encode().expect("encode");

    let (mut envelope, _): (Envelope, usize) =
      bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
    envelope.siteTag = envelope.siteTag.wrapping_add(1);
    let corrupted: Vec<u8> =
      bincode::serde::encode_to_vec(&envelope, bincode::config::standard()).unwrap();

    let result: Result<ErasedCallable, CallError> = decode(&corrupted);
    assert!( matches!(result, Err(CallError::TypeMismatch { .. })) );

    Ok(())
  }).expect("ffi! failed");
}

// ===============================================================================================