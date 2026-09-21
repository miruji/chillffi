//! Warm: the context is already open and warmed up, each sample is one operation inside it.
//!
//! `cargo bench --bench warm`. This measures the steady state, so chillffi is one retained scope
//! (`FFIScope::enter()`, opened once for the whole benchmark); an `ffi!` block behaves the same after entry.
//! What it costs to open a context is in `cold.rs`, not here.
//!
//! The callee is always libc `toupper(97) == 65`. libc is already loaded in every process involved,
//! so `dlopen` costs the same everywhere. (libm would not: Rust ships its own, Python links glibc's.)
//!
//! Scenarios (one module each, identical meaning in every row):
//! - call:       one call.
//! - payload64K: put 64 KiB into a callee-side buffer, then get the same 64 KiB back.
//!
//! payload64K is two round trips (write + ack, read request + reply): that is how chillffi's `AllocatedMemory`
//! works, and the hand-made isolated rows use the same shape.
//!
//! Two independent markers on every row:
//! - signature: ○ static (compiled in)  ·  ● dynamic (built at runtime)  ·  · probe
//! - boundary:  □ same process · ■ process boundary (fork + IPC)
//!
//! Matrix (change one axis at a time; chillffi is only ●■):
//!   ○□ static, no isolation     ○■ static + isolation
//!   ●□ dynamic, no isolation    ●■ dynamic + isolation
//!
//! - inStatic ○□:     symbol known at build time.
//! - inLibloading ○□: dlopen/dlsym at runtime, typed pointer (signature compiled in).
//! - inLibffi ●□:     signature built at runtime (the engine chillffi's worker uses).
//! - inCtypes ●□:     Python's version of the same, via pyo3.
//! - inPyCall ·□:     pyo3 calling an empty Python builtin (probe, not a competitor).
//! - isoSocket ○■:    hand-made isolation: fork + socketpair, peer calls a typed pointer.
//! - isoChillffi ●■:  retained `FFIScope` (dynamic + isolation).
//! - isoPyFork ●■:    the same hand-made isolation, written in Python.
//!
//! call: 1 IPC round trip when ■.  payload64K: 2 round trips when ■.
//!
//! Every row has the same structure, and its method body runs once:
//! 1. open the context once (a retained scope, a peer process, an embedded interpreter);
//! 2. run `warmups` unmeasured operations, because Divan does not warm up;
//! 3. hand the operation to Divan, which then times exactly `measured` samples in that same context.
//!
//! `samples` in the table is `measured`, and nothing else sets it: `warmups` never shows up there.
//! Rows of nanosecond operations show a larger `iters`: Divan puts several operations into one sample.
//! Cargo passes `--bench` only to `cargo bench`; `cargo test` and CI do not build or run this file.
//! The Python side is `support/python.py`, loaded into this process through pyo3.

#[cfg(unix)]
#[path = "support/mod.rs"]
mod support;

/// How many timed samples every row takes, in the one context it opened.
#[cfg(unix)]
const measured: u32 = 100;

#[cfg(unix)]
fn main() -> ()
{
  println!("warm: the context is already open and warmed up; one sample = one operation inside it.");
  println!("○ static signature (compiled in)    ● dynamic signature (runtime)    · probe");
  println!("□ same process                      ■ process boundary (fork + IPC)");
  println!("Matrix: ○□ static no-iso | ○■ static+iso | ●□ dynamic no-iso | ●■ dynamic+iso  ← chillffi is ●■");
  println!("Compare one axis at a time. call: 1 IPC when ■.  payload64K: 2 IPC when ■.");
  println!();
  divan::main();
}

#[cfg(not(unix))]
fn main() -> () {}

// =================================================================================================

#[cfg(unix)]
mod call
{
  use crate::support::{libcPath, loadToupper, openLibrary, python, toupper, warmUp, CFunction, Peer};
  use chillffi::ffi::library::Library;
  use chillffi::ffi::scope::FFIScope;
  use divan::{black_box, Bencher};
  use libffi::middle::{arg, Cif, CodePtr, Type};
  use pyo3::prelude::*;
  use std::ffi::c_void;

  const warmups: usize = 1_000;

  #[divan::bench(name = "inStatic ○□", sample_count = crate::measured)]
  fn inStatic(bencher: Bencher) -> ()
  {
    warmUp(warmups, || { black_box(unsafe{ toupper(black_box(97)) }); });
    bencher.bench_local(|| unsafe{ toupper(black_box(97)) });
  }

  #[divan::bench(name = "inLibloading ○□", sample_count = crate::measured)]
  fn inLibloading(bencher: Bencher) -> ()
  {
    let lib: libloading::Library = openLibrary();
    let function: CFunction = loadToupper(&lib);
    warmUp(warmups, || { black_box(unsafe{ function(black_box(97)) }); });
    bencher.bench_local(|| unsafe{ function(black_box(97)) });
  }

  #[divan::bench(name = "inLibffi ●□", sample_count = crate::measured)]
  fn inLibffi(bencher: Bencher) -> ()
  {
    let lib: libloading::Library = openLibrary();
    let symbol: libloading::Symbol<*mut c_void> = unsafe{ lib.get(b"toupper\0") }.unwrap();
    let cif: Cif = Cif::new(vec![Type::i32()], Type::i32());
    let code: CodePtr = CodePtr(*symbol);
    let operation = || {
      let value: i32 = black_box(97);
      unsafe{ cif.call::<i32>(code, &[arg(&value)]) }
    };
    warmUp(warmups, || { black_box(operation()); });
    bencher.bench_local(operation);
  }

  #[divan::bench(name = "inCtypes ●□", sample_count = crate::measured)]
  fn inCtypes(bencher: Bencher) -> ()
  {
    python(|module| {
      let toupper = module.getattr("loadToupper")?.call0()?;
      warmUp(warmups, || { black_box(toupper.call1((black_box(97),)).unwrap()); });
      bencher.bench_local(|| toupper.call1((black_box(97),)).unwrap());
      Ok(())
    });
  }

  #[divan::bench(name = "inPyCall ·□", sample_count = crate::measured)]
  fn inPyCall(bencher: Bencher) -> ()
  {
    python(|module| {
      let function = module.py().import("builtins")?.getattr("abs")?;
      warmUp(warmups, || { black_box(function.call1((black_box(97),)).unwrap()); });
      bencher.bench_local(|| function.call1((black_box(97),)).unwrap());
      Ok(())
    });
  }

  #[divan::bench(name = "isoSocket ○■", sample_count = crate::measured)]
  fn isoSocket(bencher: Bencher) -> ()
  {
    let mut peer: Peer = Peer::spawn(4, true);
    let request: [u8; 4] = 97_i32.to_ne_bytes();
    let mut reply: [u8; 4] = [0; 4];
    peer.roundTrip(&request, &mut reply);
    assert_eq!(i32::from_ne_bytes(reply), 65);
    warmUp(warmups, || peer.roundTrip(&request, &mut reply));
    bencher.bench_local(|| peer.roundTrip(&request, &mut reply));
  }

  #[divan::bench(name = "isoChillffi ●■", sample_count = crate::measured)]
  fn isoChillffi(bencher: Bencher) -> ()
  {
    let ffiScope: FFIScope = FFIScope::enter().unwrap();
    let scope = ffiScope.scope();
    let library: Library = scope.load(libcPath).unwrap();
    let value: i32 = library.call("toupper").arg::<i32>(97).result().unwrap();
    assert_eq!(value, 65);
    let operation = || library.call("toupper").arg::<i32>(black_box(97)).result::<i32>().unwrap();
    warmUp(warmups, || { black_box(operation()); });
    bencher.bench_local(operation);
  }

  #[divan::bench(name = "isoPyFork ●■", sample_count = crate::measured)]
  fn isoPyFork(bencher: Bencher) -> ()
  {
    python(|module| {
      let (operation, finish) = module.getattr("makeForkCall")?.call0()?.extract::<(Bound<'_, PyAny>, Bound<'_, PyAny>)>()?;
      warmUp(warmups, || { operation.call0().unwrap(); });
      bencher.bench_local(|| operation.call0().unwrap());
      finish.call0()?;
      Ok(())
    });
  }
}

// =================================================================================================

#[cfg(unix)]
mod payload64K
{
  use crate::support::{payloadSize, python, warmUp, BufferPeer};
  use chillffi::ffi::allocatedMemory::AllocatedMemory;
  use chillffi::ffi::scope::FFIScope;
  use divan::{black_box, Bencher};
  use pyo3::prelude::*;

  const warmups: usize = 1;

  /// In-process floor: copy 64 KiB in, copy 64 KiB out.
  #[divan::bench(name = "inStatic ○□", sample_count = crate::measured)]
  fn inStatic(bencher: Bencher) -> ()
  {
    let payload: Vec<u8> = vec![0xA5; payloadSize];
    let mut buffer: Vec<u8> = vec![0; payloadSize];
    let mut operation = || {
      buffer.copy_from_slice(black_box(&payload));
      black_box(&buffer).to_vec()
    };
    warmUp(warmups, || { black_box(operation()); });
    bencher.bench_local(operation);
  }

  #[divan::bench(name = "inCtypes ●□", sample_count = crate::measured)]
  fn inCtypes(bencher: Bencher) -> ()
  {
    python(|module| {
      let operation = module.getattr("makeCtypesPayload")?.call0()?;
      warmUp(warmups, || { black_box(operation.call0().unwrap()); });
      bencher.bench_local(|| operation.call0().unwrap());
      Ok(())
    });
  }

  #[divan::bench(name = "isoSocket ○■", sample_count = crate::measured)]
  fn isoSocket(bencher: Bencher) -> ()
  {
    let mut peer: BufferPeer = BufferPeer::spawn(payloadSize);
    let mut request: Vec<u8> = vec![0xA5; payloadSize + 1];
    request[0] = 1;
    let mut reply: Vec<u8> = vec![0; payloadSize];
    let mut operation = || {
      peer.write(&request);
      peer.read(&mut reply);
    };
    warmUp(warmups, &mut operation);
    bencher.bench_local(operation);
  }

  /// `write` takes ownership of a Vec, so the clone belongs to the API cost.
  #[divan::bench(name = "isoChillffi ●■", sample_count = crate::measured)]
  fn isoChillffi(bencher: Bencher) -> ()
  {
    let payload: Vec<u8> = vec![0xA5; payloadSize];
    let ffiScope: FFIScope = FFIScope::enter().unwrap();
    let scope = ffiScope.scope();
    let mem: AllocatedMemory = scope.alloc(payloadSize).unwrap();
    mem.write(payload.clone()).unwrap();
    assert_eq!(mem.read().unwrap(), payload);
    let operation = || {
      mem.write(payload.clone()).unwrap();
      mem.read().unwrap()
    };
    warmUp(warmups, || { black_box(operation()); });
    bencher.bench_local(operation);
  }

  #[divan::bench(name = "isoPyFork ●■", sample_count = crate::measured)]
  fn isoPyFork(bencher: Bencher) -> ()
  {
    python(|module| {
      let (operation, finish) = module.getattr("makeForkPayload")?.call0()?.extract::<(Bound<'_, PyAny>, Bound<'_, PyAny>)>()?;
      warmUp(warmups, || { operation.call0().unwrap(); });
      bencher.bench_local(|| operation.call0().unwrap());
      finish.call0()?;
      Ok(())
    });
  }
}
