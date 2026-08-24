//! **A simple isolated dynamic FFI framework for Rust.**
//!
//! `chillffi` allows dynamically loading C libraries `.so` and calling their
//! functions at runtime, **isolating the calls in a separate empty process**.
//!
//! If third-party C code crashes or corrupts something, your main Rust
//! application will continue running.
//!
//! _(In the future, an expansion of the functionality for working with FFI is
//! planned.)_
//!
//! # Platform support
//!
//! Supported only on Unix-like OSes, and tested only on Linux.
//!
//! _(Planned: Windows, macOS, WASM, Bare metal.)_
//!
//! # Features
//!
//! - **Crash Isolation**: A crash or panic inside unreliable FFI code does not
//!   break or corrupt the main process.
//! - **Zygote Model**: Fast forking and spawning of isolated workers with
//!   minimal overhead.
//! - **In-memory IPC**: Transfer of file descriptors and data through sockets
//!   without accessing the disk.
//! - **Dynamic FFI**: On-the-fly function calls without the need to compile
//!   static C bindings.
//!
//! # Quick start
//!
//! ```no_run
//! use chillffi::ffi::value::{Value};
//! use chillffi::call;
//! use chillffi::ffi;
//! 
//! fn main() -> ()
//! {
//!   // Perform an FFI call inside an isolated context using a macro
//!   let result: f64 = ffi!{
//!     // Dynamically load the system library
//!     let libm: Library = Library::load("libm.so.6")?;
//!   
//!     // Call the "sqrt" function, specifying the expected return type
//!     Ok(call!(libm, "sqrt", 4.0 as f64)?)
//!     
//!     // Here libm will be automatically cleared due to drop() when exiting the closure.
//!     // You can also do this manually via drop(libm) or libm.unload()?
//!   }.expect("FFI call failed");
//!
//!   // Process the typed result
//!   println!("sqrt(4.0) = {}", result);
//!   assert!((result - 2.0).abs() < f64::EPSILON, "sqrt(4.0) != 2.0");
//! }
//! ```
//!
//! For memory-sensitive operations — C strings, out-parameters, or raw
//! buffers — use the scoped variant with [`Scope`](crate::ffi::scope::Scope)
//! and [`AllocatedMemory`](crate::ffi::allocatedMemory::AllocatedMemory):
//!
//! ```no_run
//! use chillffi::ffi::allocatedMemory::{AllocatedMemory};
//! use chillffi::ffi::value::{Value};
//! use chillffi::ffi::errors::FFIError;
//! use chillffi::callv;
//! use chillffi::ffi;
//! 
//! fn main() -> ()
//! {
//!   // clock_gettime(CLOCK_REALTIME, &timespec) — struct out-param via Alloc/ReadMemory,
//!   // the case a plain Value::Pointer can't cover on its own.
//!   let (secs, nanos): (i64, i64) = ffi!(|scope| {
//!     let libc: Library = Library::load("libc.so.6")?;
//!
//!     // struct timespec { time_t tv_sec; long tv_nsec; } — 16 bytes on x86_64 Linux
//!     let mem: AllocatedMemory = scope.alloc(16)?;
//!
//!     callv!(libc, "clock_gettime", 0 as i32 /* CLOCK_REALTIME */, mem.asPointer())?;
//!
//!     let Value::RawString(bytes) = mem.read()? else { 
//!       panic!("expected bytes")
//!     };
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
//! # Modules
//!
//! - [`mod@ffi`] — public API: [`Library`](crate::ffi::library::Library),
//!   [`Value`](crate::ffi::value::Value), [`Type`](crate::ffi::value::Type),
//!   [`Scope`](crate::ffi::scope::Scope),
//!   [`AllocatedMemory`](crate::ffi::allocatedMemory::AllocatedMemory),
//!   and error types.
//!
//! # License
//!
//! The source code is distributed under the FCL license.
//! See the repository for the full text.

// =================================================================================================

#[cfg(not(target_os = "linux"))]
compile_error!("chillffi supports only Linux operating systems.");
// Currently available only on Linux, although it should work on UNIX in general.
// But I have not tested it on macOS.

// =================================================================================================

mod worker;
mod zygote;
pub mod ffi;
pub mod pathResolver;
// =================================================================================================

use std::{env};
use crate::zygote::{initZygote, runAsZygote, ZygoteFlag};

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
  }

  // Do this once to start the main zygote
  initZygote().expect("Failed to setup zygote");
}

// =================================================================================================

/// Internal items re-exported for the `ffi!` macro.  
/// Not part of the public API; do not use directly.
#[doc(hidden)]
pub mod __ffiInternal {
  pub use crate::zygote::{ClonedZygote, ZygoteGuard};
}

/// Main macro for working with FFI.
///
/// It creates a copy of the zygote from the main zygote.
///
/// After that, any FFI code can be executed inside it.
///
/// Library specifically blocks FFI calls outside this macro.
///
/// Isolation allows adding FFI insertions without breaking or corrupting the main runtime.
///
/// todo
///  Important: It will take ownership of the Library type — therefore, the code inside
///  will have to specify it differently when this data type matches. However, this will
///  be quite rare, because FFI insertions should be rare and it is not guaranteed that
///  exactly Library will end up there.
///  The simplest solution would be for the user to rename the type — then they will not
///  see errors for their Library type.
#[macro_export]
macro_rules! ffi 
{
  // Variant with access to Scope. `ffi!(|scope| {})`. 
  // The scope name can be any name — the important thing is that there are no repetitions inside {}.
  // Scope<'g> borrows the ScopeGuard of this block, therefore AllocatedMemory<'g>
  // cannot be returned outside — the compiler catches this, not us.
  (|$scopeName:ident| { $($body:tt)* }) => 
  {
    (|| -> Result<_, $crate::ffi::errors::FFIError> 
    {
      #[allow(unused_imports)]
      use $crate::ffi::library::__FFILibrary as Library;
 
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
 
  // Variant without Scope, ScopeGuard is not created at all.
  ($($body:tt)*) =>
  {
    (|| -> Result<_, $crate::ffi::errors::FFIError> 
    {
      #[allow(unused_imports)]
      use $crate::ffi::library::__FFILibrary as Library;
 
      // Creating a clone-zygote from the main one
      let zygote = $crate::__ffiInternal::ClonedZygote::getMeClone()?;
 
      // Registering the clone-zygote in the current thread's ZygoteStack
      let _guard = $crate::__ffiInternal::ZygoteGuard::enter(zygote);
 
      // Executing the body
      $($body)*
    })()
  };

}

// =================================================================================================