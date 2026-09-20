//! A low-level platform-dependent layer for:
//! - The lifetime of the process
//! - The base address of the module,
//! - `errno`,
//! - The low-level allocator underlying `Alloc`/`Free`,
//! - Other stuff.
// =================================================================================================

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use self::unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(not(any(unix, windows)))]
compile_error!("chillffi supports Unix-like OSes and Windows only");

// =================================================================================================

/// Process id, normalized to what `std::process::Child::id` already returns.
pub type ProcessId = u32;

/// posix_memalign / _aligned_malloc both require at least a pointer's worth.
pub const MinAlignment: usize = size_of::<*mut core::ffi::c_void>();

// =================================================================================================
