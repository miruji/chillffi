//! Global default controlling whether FFI calls read `errno` right after
//! they return.
//! 
//! Priority (most specific wins):
//! 1. per-call override — [`crate::ffi::library::CallBuilder::errno`] / [`crate::ffi::library::CallBuilder::noErrno`]
//!    or [`crate::ffi::scope::Scope::callPointerErrno`]
//! 2. scope-level override — [`crate::ffi::scope::Scope::setReadErrno`]
//! 3. this global default — [`setGlobalReadErrno`]
// =================================================================================================
use std::sync::atomic::{AtomicBool, Ordering};
// =================================================================================================

/// Global default: `false` (no capture, no overhead) unless explicitly enabled.
static GlobalReadErrno: AtomicBool = AtomicBool::new(false);

/// Sets the global default for errno capture. Affects only calls that don't
/// specify their own scope or per-call override.
pub fn setGlobalReadErrno(enabled: bool) -> ()
{
  GlobalReadErrno.store(enabled, Ordering::Relaxed);
}

/// Reads the current global default for errno capture.
pub(super) fn globalReadErrno() -> bool
{
  GlobalReadErrno.load(Ordering::Relaxed)
}

// =================================================================================================

#[cfg(test)]
mod tests
{
  use super::*;
  // ===============================================================================================

  /// Checks that the global default is off unless explicitly enabled, and
  /// that enabling/disabling it round-trips through the atomic.
  #[test]
  fn globalDefaultRoundtrip() -> ()
  {
    setGlobalReadErrno(true);
    assert!(globalReadErrno());

    setGlobalReadErrno(false);
    assert!(!globalReadErrno());
  }

  // ===============================================================================================
}

// =================================================================================================