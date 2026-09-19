//! **A simple isolated dynamic FFI framework for Rust.**
//!
//! `chillffi` allows dynamically loading **C ABI-compatible libraries** (`.so`, `.dylib`, `.dll`)
//! and calling their functions at runtime, **isolating each FFI call in a separate process**.
//! If third-party native code crashes or corrupts memory, 
//! the failure is contained within the isolated process, 
//! keeping your main Rust application running.
//!
//! # Platform support
//!
//! Unix-like OSes and Windows (MSVC toolchain — `libffi-sys` has no `gnu` build).
//! On Unix a clone is a `fork` of the main zygote; on Windows it's a fresh
//! process re-running the same executable, since there is no `fork`.
//!
//! # Quick start
//!
//! ```no_run
//! use chillffi::ffi;
//!
//! fn main() -> ()
//! {
//!   // Perform an FFI call inside an isolated context using a macro
//!   let result: f64 = ffi!(|scope| {
//!     // Dynamically load the system library, bound to this scope
//!     let libm: Library = scope.load("libm.so.6")?;
//!   
//!     // Call the "sqrt" function, specifying the expected return type
//!     libm.call("sqrt").arg::<f64>(4.0).result()
//!     
//!     // Here libm will be automatically cleared due to drop() when exiting the closure.
//!     // You can also do this manually via drop(libm) or libm.unload()?
//!   }).expect("FFI call failed");
//!
//!   // Process the typed result
//!   println!("sqrt(4.0) = {}", result);
//!   assert!((result - 2.0).abs() < f64::EPSILON, "sqrt(4.0) != 2.0");
//! }
//! ```
//! 
//! Example of a memory-sensitive call to the `clock_gettime` function 
//! from the system library `libc.so.6` using [`AllocatedMemory`](ffi::allocatedMemory::AllocatedMemory):
//!
//! ```no_run
//! use chillffi::ffi::allocatedMemory::{AllocatedMemory};
//! use chillffi::ffi::errors::FFIError;
//! use chillffi::ffi;
//!
//! fn main() -> ()
//! {
//!   // clock_gettime(CLOCK_REALTIME, &timespec) — struct out-param via Alloc/ReadMemory,
//!   // the case a plain Value::Pointer can't cover on its own.
//!   let (secs, nanos): (i64, i64) = ffi!(|scope| {
//!     let libc: Library = scope.load("libc.so.6")?;
//!
//!     // struct timespec { time_t tv_sec; long tv_nsec; } — 16 bytes on x86_64 Linux
//!     let mem: AllocatedMemory = scope.alloc(16)?;
//!
//!     libc.call("clock_gettime")
//!       .arg::<i32>(0 /* CLOCK_REALTIME */)
//!       .arg(mem.asPointer())
//!       .void()?;
//!
//!     let bytes: Vec<u8> = mem.read()?;
//!     drop(mem);
//!
//!     let secs: i64 = i64::from_ne_bytes(bytes[0..8].try_into().unwrap());
//!     let nanos: i64 = i64::from_ne_bytes(bytes[8..16].try_into().unwrap());
//!     Ok((secs, nanos))
//!   }).expect("clock_gettime failed");
//!
//!   println!("clock_gettime(CLOCK_REALTIME) = {}.{:09}", secs, nanos);
//! }
//! ```
//!
//! For more detailed examples, see the `examples` folder.
//! 
//! The tests there are divided by features, and inside there are different usage variations.
//!
//! You can also run them via `cargo run --example <name>`.
//!
//! # Why is this convenient
//!
//! In general practice, we are used to doing it like in Python and other
//! programming languages — precisely specifying all the wrappers for FFI.
//! After which we observe how FFI still crashes anyway and the libraries are
//! not built, and the code does not work.
//!
//! This is all because FFI requires a manual bridge and it is not always
//! possible to make one.
//!
//! **chillffi** works on a different principle — you can write any FFI code
//! inside isolated blocks. Because FFI should not be scattered throughout your
//! code — this is an unsafe approach. Therefore, we write it in isolation and
//! preferably briefly, only when necessary.
//!
//! Since everything is located in isolated processes — we do not damage the
//! main runtime in any way and do not touch your code. All FFI requests work
//! in a sterile manner and in case of errors will clearly let you know about
//! it. You can also simply ignore them if you want.
//!
//! As a result, we can freely and simply write:
//! - Test code
//! - Educational code
//! - FFI bridges
//! - Dynamic programming languages
//! - Game engines
//! - Reactive systems and dynamic systems
//! - And many other things
//!
//! This is also different from the WASM approach — because we preserve a true
//! native execution here.
//!
//! # How it works
//!
//! 1. Before your code starts running, a Zygote is created — it is an empty
//!    process for cloning itself and isolating FFI.
//! 2. When work with FFI is required — a copy is created from the zygote.
//! 3. Data and descriptors are transferred through a secure socket channel in
//!    memory.
//! 4. In case of errors, the supervisor intercepts the worker crash and returns
//!    the error to Rust, keeping your application stable.
//!
//! <div class="warning">
//!
//! This does not protect you from the FFI code running inside the isolated process.
//!
//! For example, if it does something with your OS or file system -
//! it is already your responsibility to separately protect against this.
//!
//! For example: You can use a virtual space for the file system and so on.
//!
//! </div>
//!
//! <div class="warning">
//!
//! FFI blocks should be as small as possible in size. I.e., not 100 lines in 1 FFI space.
//!
//! An exception can be considered when you need a single address space for several operations.
//!
//! In other cases, you should separate FFI requests as much as possible.
//!
//! Because no one can guarantee that any FFI request will not break your code.
//!
//! Even if you are an experienced programmer, there are things that do not depend on your experience.
//!
//! </div>
//!
//! # License
//!
//! The source code is distributed under the FCL license.
//! See the repository for the full text.

// =================================================================================================

/// Used for running tests.
#[cfg(test)]
mod examplesPlatform 
{
  include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/platform/mod.rs"));
}

// =================================================================================================

mod worker;
mod zygote;
mod platform;
pub mod ffi;
pub mod pathResolver;
pub mod errnoPolicy;

// =================================================================================================

use std::{env};
use crate::zygote::{initZygote, runAsZygote, ZygoteFlag};
#[cfg(windows)]
use crate::platform::ipc::windows::CloneFlag;
#[cfg(windows)]
use crate::zygote::{runAsClone};

// =================================================================================================

/// Single entry point for zygote initialization in any binary (including tests).
/// Checks whether the process is running as a zygote; if so — switches to daemon mode,
/// otherwise — initializes the parent side.
#[ctor::ctor(unsafe)]
fn zygoteEntrypoint() -> ()
{
  let mut args = env::args_os();
  args.next();
  if let Some(arg) = args.next()
  {
    if arg == ZygoteFlag
    {
      runAsZygote();
    }

    #[cfg(windows)]
    if arg == CloneFlag
    {
      runAsClone();
    }
  }

  // Do this once to start the main zygote
  initZygote().expect("Failed to setup zygote");
}

// =================================================================================================

/// Internal items re-exported for the [`ffi!`] macro.  
/// Not part of the public API; do not use directly.
#[doc(hidden)]
pub mod __ffiInternal 
{
  pub use crate::zygote::{ClonedZygote, ZygoteGuard};
}

/// Main macro for working with FFI.
///
/// It creates a copy of the zygote from the main zygote and opens a [`Scope`](crate::ffi::scope::Scope)
/// bound to it — `scope` is how you load libraries ([`Scope::load`](crate::ffi::scope::Scope::load))
/// and allocate memory ([`Scope::alloc`](crate::ffi::scope::Scope::alloc)) for the duration of the block.
///
/// `Library<'g>` can only be constructed via `scope.load(...)`, and only lives as long as the
/// scope that produced it — the compiler enforces this, not us. There is no variant of this
/// macro without a scope: an FFI block always needs one to load anything into.
///
/// Isolation allows adding FFI insertions without breaking or corrupting the main runtime.
#[macro_export]
macro_rules! ffi
{
  // `ffi!(|scope| { ... })`. The scope name can be any identifier — the important
  // thing is that there are no repetitions inside {}. Scope<'g> borrows the
  // ScopeGuard of this block, therefore AllocatedMemory<'g> and Library<'g>
  // cannot be returned outside — the compiler catches this, not us.
  (|$scopeName:ident| { $($body:tt)* }) => 
  {
    (|| -> Result<_, $crate::ffi::errors::FFIError> 
    {
      #[allow(unused_imports)]
      use $crate::ffi::library::Library;
 
      // Creating a clone-zygote from the main one
      let zygote = $crate::__ffiInternal::ClonedZygote::getMeClone()?;
 
      // Registering the clone-zygote in the current thread's ZygoteStack
      let _guard = $crate::__ffiInternal::ZygoteGuard::enter(zygote);
 
      // ScopeGuard lives strictly within the boundaries of this block; $scopeName borrows it.
      let _scopeGuard = $crate::ffi::scope::ScopeGuard::new();
      let $scopeName = $crate::ffi::scope::Scope::new(&_scopeGuard);
 
      // Executing the body
      $($body)*
    })()
  };
}

// =================================================================================================