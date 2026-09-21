//! Cold: every sample opens a fresh context, loads the library, calls once, and closes the context.
//!
//! `cargo bench --bench cold`. This is what `ffi!` costs when it is used the way it is meant to be used
//! for a single operation. Steady-state cost inside an open scope is in `warm.rs`, not here.
//!
//! What an isolated row pays per sample:
//! - process creation (chillffi: a clone forked by the zygote, which already exists),
//! - the IPC handshake,
//! - loading the library and one call (chillffi: libffi in the worker),
//! - teardown.
//!
//! What an in-process row pays: load + call + close, no process. They are floors, not peers.
//! Where the fork comes from differs: isoSocket and isoPyFork fork this process (divan, pyo3, chillffi's
//! supervisor thread and possibly an initialised Python inside it); chillffi forks its own zygote,
//! which is a separate process started from the same executable and never runs benchmark code.
//!
//! The callee is always libc `toupper(97) == 65`. libc is already loaded in every process involved,
//! so `dlopen` costs the same everywhere. (libm would not: Rust ships its own, Python links glibc's.)
//! "Close" is dlclose when in-process, and the death of the worker when isolated.
//!
//! Two independent markers on every row:
//! - signature: ○ static (compiled in) · ● dynamic (built at runtime)
//! - boundary:  □ same process · ■ process boundary (fork + IPC)
//!
//! Matrix (change one axis at a time; chillffi is only ●■):
//!   ○□ static, no isolation     ○■ static + isolation
//!   ●□ dynamic, no isolation    ●■ dynamic + isolation
//!
//! - inLibloading ○□: dlopen/dlsym at runtime, typed pointer (signature compiled in).
//! - inLibffi ●□:     signature built at runtime.
//! - inCtypes ●□:     Python's version of the same, via pyo3.
//! - isoSocket ○■:    hand-made isolation: fork + socketpair per operation, child calls a typed pointer.
//! - isoChillffi ●■:  `ffi!` block (dynamic + isolation).
//! - isoFFIScope ●■:  `FFIScope::enter()` per operation, the second way in.
//! - isoPyFork ●■:    the same hand-made isolation, written in Python.
//!
//! Every row runs its method body once: `warmups` unmeasured operations (Divan does not warm up),
//! then Divan times exactly `measured` samples, and each sample is a whole fresh context.
//! `samples` in the table is `measured`, and nothing else sets it.
//! Cargo passes `--bench` only to `cargo bench`; `cargo test` and CI do not build or run this file.
//! The Python side is `support/python.py`, loaded into this process through pyo3.

#[cfg(unix)]
#[path = "support/mod.rs"]
mod support;

/// How many timed samples every row takes; each sample opens and closes a whole context.
#[cfg(unix)]
const measured: u32 = 300;

#[cfg(unix)]
fn main() -> ()
{
  println!("cold: every sample = fresh context + load + one call + close.");
  println!("○ static signature (compiled in)    ● dynamic signature (runtime)");
  println!("□ same process (floors: load+call+close)   ■ process boundary (fork+IPC+load+call+teardown)");
  println!("Matrix: ○□ static no-iso | ○■ static+iso | ●□ dynamic no-iso | ●■ dynamic+iso  ← chillffi is ●■");
  println!("isoSocket/isoPyFork fork this process; chillffi forks its own zygote (separate, small process).");
  println!("Compare one axis at a time.");
  println!();
  divan::main();
}

#[cfg(not(unix))]
fn main() -> () {}

// =================================================================================================

#[cfg(unix)]
mod oneShot
{
  use crate::support::{libcPath, loadToupper, openLibrary, python, warmUp};
  use chillffi::ffi;
  use chillffi::ffi::errors::FFIError;
  use chillffi::ffi::scope::FFIScope;
  use divan::{black_box, Bencher};
  use libffi::middle::{arg, Cif, CodePtr, Type};
  use pyo3::prelude::*;
  use std::ffi::c_void;
  use std::io::{Read, Write};
  use std::os::unix::net::UnixStream;

  const warmups: usize = 20;

  #[divan::bench(name = "inLibloading ○□", sample_count = crate::measured)]
  fn inLibloading(bencher: Bencher) -> ()
  {
    let operation = || {
      let lib: libloading::Library = openLibrary();
      let function = loadToupper(&lib);
      unsafe{ function(black_box(97)) }
    };
    warmUp(warmups, || { black_box(operation()); });
    bencher.bench_local(operation);
  }

  #[divan::bench(name = "inLibffi ●□", sample_count = crate::measured)]
  fn inLibffi(bencher: Bencher) -> ()
  {
    let operation = || {
      let lib: libloading::Library = openLibrary();
      let symbol: libloading::Symbol<*mut c_void> = unsafe{ lib.get(b"toupper\0") }.unwrap();
      let cif: Cif = Cif::new(vec![Type::i32()], Type::i32());
      let value: i32 = black_box(97);
      unsafe{ cif.call::<i32>(CodePtr(*symbol), &[arg(&value)]) }
    };
    warmUp(warmups, || { black_box(operation()); });
    bencher.bench_local(operation);
  }

  #[divan::bench(name = "inCtypes ●□", sample_count = crate::measured)]
  fn inCtypes(bencher: Bencher) -> ()
  {
    python(|module| {
      let function = module.getattr("ctypesOneShot")?;
      warmUp(warmups, || { black_box(function.call0().unwrap()); });
      bencher.bench_local(|| function.call0().unwrap());
      Ok(())
    });
  }

  /// Fork per operation: the child loads the library, calls, answers, dies.
  #[divan::bench(name = "isoSocket ○■", sample_count = crate::measured)]
  fn isoSocket(bencher: Bencher) -> ()
  {
    let operation = || {
      let (mut left, mut right): (UnixStream, UnixStream) = UnixStream::pair().unwrap();
      let pid: i32 = unsafe{ libc::fork() };
      if pid == 0
      {
        drop(left);
        let lib: libloading::Library = openLibrary();
        let function = loadToupper(&lib);
        let _ = right.write_all(&unsafe{ function(97) }.to_ne_bytes());
        unsafe{ libc::_exit(0) };
      }
      drop(right);
      let mut bytes: [u8; 4] = [0; 4];
      left.read_exact(&mut bytes).unwrap();
      unsafe{ libc::waitpid(pid, std::ptr::null_mut(), 0) };
      i32::from_ne_bytes(bytes)
    };
    warmUp(warmups, || { black_box(operation()); });
    bencher.bench_local(operation);
  }

  #[divan::bench(name = "isoChillffi ●■", sample_count = crate::measured)]
  fn isoChillffi(bencher: Bencher) -> ()
  {
    let operation = || {
      let value: i32 = ffi!(|scope| {
        let library: Library = scope.load(libcPath)?;
        library.call("toupper").arg::<i32>(97).result()
      }).expect("chillffi block failed");
      value
    };
    warmUp(warmups, || { black_box(operation()); });
    bencher.bench_local(operation);
  }

  #[divan::bench(name = "isoFFIScope ●■", sample_count = crate::measured)]
  fn isoFFIScope(bencher: Bencher) -> ()
  {
    use chillffi::ffi::library::Library;
    let operation = || {
      let value: i32 = (|| -> Result<i32, FFIError> {
        let ffiScope: FFIScope = FFIScope::enter()?;
        let scope = ffiScope.scope();
        let library: Library = scope.load(libcPath)?;
        library.call("toupper").arg::<i32>(97).result()
      })().expect("FFIScope failed");
      value
    };
    warmUp(warmups, || { black_box(operation()); });
    bencher.bench_local(operation);
  }

  #[divan::bench(name = "isoPyFork ●■", sample_count = crate::measured)]
  fn isoPyFork(bencher: Bencher) -> ()
  {
    python(|module| {
      let function = module.getattr("forkOneShot")?;
      warmUp(warmups, || { black_box(function.call0().unwrap()); });
      bencher.bench_local(|| function.call0().unwrap());
      Ok(())
    });
  }
}
