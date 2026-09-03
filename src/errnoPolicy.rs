//! Global default controlling whether FFI calls read `errno` right after
//! they return. A scope-level override ([`Scope::setReadErrno`](crate::ffi::scope::Scope::setReadErrno))
//! or a per-call override (`CallBuilder::errno`/`noErrno`, `Scope::callPointerErrno`)
//! takes priority over this — see [`crate::ffi::library::resolveReadErrno`].
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