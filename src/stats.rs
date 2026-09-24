//! Live process counters — the runtime view of the pools declared in
//! [`crate::limits`].
//!
//! Each function here returns a count of processes that are alive right
//! now, in the categories the user is most likely to want to monitor:
//!
//! - [`activeClones`]: how many `ffi!{}` clones are currently running.
//!   Bounded above by `limits::current().concurrencyLimit`.
//! - [`availableSlots`]: how many more clones can start immediately
//!   without blocking. `concurrencyLimit - activeClones`.
//! - [`zygotesAlive`]: how many Main Zygote processes are alive (1
//!   today; > 1 reserved for future multi-zygote work, see the `todo`
//!   at the top of `src/zygote.rs`).
//! - [`hotAlive`]: how many warm clone processes are kept between
//!   `ffi!{}` blocks. Always 0 today; reserved for the future
//!   retained-scope API.
//! - [`configured`]: the active [`Limits`](crate::limits::Limits)
//!   struct, for "active vs limit" comparison without a separate call.
//!
//! All functions are cheap (one atomic load or one `OnceLock` get + a
//! copy of a small struct) and lock-free where it matters — safe to
//! call from a hot path or a debug dashboard.
//!
//! # Quick start
//!
//! ```no_run
//! println!(
//!   "clones: {}/{} ({} free), zygotes: {}, hot: {}",
//!   chillffi::stats::activeClones(),
//!   chillffi::stats::configured().concurrencyLimit,
//!   chillffi::stats::availableSlots(),
//!   chillffi::stats::zygotesAlive(),
//!   chillffi::stats::hotAlive()
//! );
//! ```
// =================================================================================================
use std::sync::atomic::{AtomicU64, Ordering};
// =================================================================================================

// Live clones counter — incremented on every successful
// `crate::zygote::ClonedZygote::getMeClone`, decremented on `Drop`.
// Lock-free.
static ACTIVE_CLONES: AtomicU64 = AtomicU64::new(0);

// Live Main Zygote counter — incremented on `crate::zygote::initZygote`
// success, decremented when the supervisor retires a dead zygote (or
// the process exits). Lock-free.
static ZYGOTES_ALIVE: AtomicU64 = AtomicU64::new(0);

// =================================================================================================

/// Internal hook called by [`crate::zygote::ClonedZygote::getMeClone`]
/// right after a clone has been spawned. Bumps [`activeClones`].
pub(super) fn onCloneSpawned() -> ()
{
  ACTIVE_CLONES.fetch_add(1, Ordering::AcqRel);
}

/// Internal hook called by `ClonedZygote::drop` right before killing
/// the clone process. Decrements [`activeClones`].
pub(super) fn onCloneDropped() -> ()
{
  // `fetch_sub` with `AcqRel` so a concurrent reader sees the decrement
  // happen-before the kill.
  ACTIVE_CLONES.fetch_sub(1, Ordering::AcqRel);
}

/// Internal hook called by [`crate::zygote::initZygote`] on success.
pub(super) fn onZygoteSpawned() -> ()
{
  ZYGOTES_ALIVE.fetch_add(1, Ordering::AcqRel);
}

/// Internal hook called when the supervisor retires a dead Main Zygote.
pub(super) fn onZygoteRetired() -> ()
{
  ZYGOTES_ALIVE.fetch_sub(1, Ordering::AcqRel);
}

// =================================================================================================

/// Number of `ffi!{}` clones currently alive in this chillffi process,
/// summed across all threads. Bounded above by
/// `limits::current().concurrencyLimit`.
///
/// Lock-free: a single `AtomicU64::load(Acquire)`.
pub fn activeClones() -> u64
{
  ACTIVE_CLONES.load(Ordering::Acquire)
}

/// Number of Main Zygote processes currently alive (today: 1).
///
/// Lock-free.
pub fn zygotesAlive() -> u64
{
  ZYGOTES_ALIVE.load(Ordering::Acquire)
}

/// Number of warm clone processes kept between `ffi!{}` blocks (today: always 0).
///
/// Tracked under the same concurrency limit as the per-block clones;
/// today the entire `hotPool` is reserved for future work, so this is
/// always 0. Provided so dashboards built against this API stay stable
/// when the future hot-pool lands.
pub const fn hotAlive() -> u64 { 0 }

/// Number of additional clones that can start immediately without
/// blocking on the concurrency limit.
///
/// `concurrencyLimit - activeClones`. Lock-free.
pub fn availableSlots() -> u64
{
  // `availableSlots` reads two atomics; treat them as a snapshot, not
  // as a consistent pair. The two reads are independent and the worst a
  // race can do is report a number that is off by the number of clones
  // that started or finished in between — which is a meaningful number
  // for a dashboard only if the dashboard also tolerates it.
  let limit: u64 = crate::limits::activeLimitPublic();
  let active: u64 = activeClones();
  limit.saturating_sub(active)
}

/// The active [`Limits`](crate::limits::Limits) struct, for "active vs
/// limit" comparison without a separate call. Same as
/// [`crate::limits::current`] but re-exposed here so a dashboard has
/// everything in one module.
pub fn configured() -> crate::limits::Limits
{
  *crate::limits::current()
}

// =================================================================================================

#[cfg(test)]
mod tests
{
  use crate::limits::availableSlots;
  use crate::stats::{activeClones, configured, hotAlive, zygotesAlive};

  // ===============================================================================================

  /// Smoke check that `activeClones` is callable and returns a sensible
  /// value when no `ffi!{}` block is in flight (0 or a tiny residual
  /// number, given the test process is the only thing running). It does
  /// not assert the exact value because the rest of the test binary may
  /// have already created clones that have not yet been reaped — the
  /// guarantee is "the counter is alive and returns a number".
  #[test]
  fn activeClonesIsCallable() -> ()
  {
    let n: u64 = activeClones();
    // The counter must never underflow; we use saturating_sub elsewhere,
    // but the underlying atomic must never be negative.
    assert!(n < u64::MAX / 2, "activeClones is not a runaway counter");
  }

  /// `availableSlots` must equal `concurrencyLimit - activeClones`
  /// (saturating) at the time of the call. We only check the
  /// saturating-subtraction invariant, not an exact number, because the
  /// test process may have in-flight clones from other tests.
  #[test]
  fn availableSlotsMatchesLimitMinusActive() -> ()
  {
    let limit: u64 = configured().concurrencyLimit;
    let active: u64 = activeClones();
    let avail: u64 = availableSlots();
    assert_eq!(avail, limit.saturating_sub(active));
  }

  /// `zygotesAlive` must be at least 1 — the Main Zygote was started
  /// by the `ctor` before any test ran.
  #[test]
  fn zygotesAliveIsAtLeastOne() -> ()
  {
    assert!(zygotesAlive() >= 1);
  }

  /// `hotAlive` is always 0 today — there is no hot-pool yet.
  #[test]
  fn hotAliveIsZeroToday() -> ()
  {
    assert_eq!(hotAlive(), 0);
  }

  // ===============================================================================================
}

// =================================================================================================
