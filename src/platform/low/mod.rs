//! OS layer for process lifetime, module base address, `errno` and the
//! raw allocator behind `Alloc`/`Free`. Everything above this module
//! (`zygote`, `worker`, `ffi`) stays platform-neutral.
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
