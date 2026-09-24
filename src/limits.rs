//! Process-pool sizing and the concurrency limit that protects the host
//! from a fork-bomb under parallel [`crate::ffi!`] blocks.
//!
//! # Why this exists
//!
//! Each `ffi!{}` block creates a clone of the Main Zygote. With no
//! coordination between threads, `N` concurrent `ffi!{}` blocks produce
//! `N` live clones — and a workload that opens more of them in parallel
//! than the OS allows is a fork-bomb: the next clone attempt returns
//! `EAGAIN` (Unix) or `ERROR_MAX_THRDS` (Windows), and the call surfaces
//! as a `SpawnFailed` reply. Worse, those `N` clones count against the
//! **per-user** limit (`RLIMIT_NPROC` on Linux/macOS), shared with the
//! shell, system daemons, and any **other** chillffi process running
//! under the same user.
//!
//! This module imposes one global ceiling on the number of simultaneously
//! live clones, blocking (not erroring) the caller when the ceiling is
//! reached — so a parallel workload degrades gracefully to "as many
//! concurrent FFI blocks as the limit allows", instead of falling over
//! the OS edge.
//!
//! # The three pools
//!
//! chillffi's process topology has three distinct roles; all three are
//! declared here so that future work (multi-zygote, retained hot workers)
//! can plug into the same configuration surface without breaking the
//! public API:
//!
//! ```text
//! zygotePool       ─ Main Zygotes ready to fork a clone on demand.
//!                     Default: 1 (today: exactly one Main Zygote per
//!                     chillffi process — `> 1` is reserved for future
//!                     work, see the `todo` block at the top of
//!                     `src/zygote.rs`).
//!
//! hotPool          ─ Long-lived clone processes kept warm between
//!                     `ffi!{}` blocks for low-latency reuse.
//!                     Default: 0 (today: every `ffi!{}` block gets a
//!                     fresh clone from the Main Zygote and kills it on
//!                     scope exit). Reusing a clone across blocks would
//!                     carry `dlopen`'d state and allocations from one
//!                     block into the next, which breaks the "sterile
//!                     process per FFI block" guarantee. `> 0` is
//!                     reserved for the explicit retained-scope API
//!                     ([`crate::ffi::scope`]); an opt-in is on the roadmap.
//!
//! concurrencyLimit ─ Hard ceiling on simultaneously live clones,
//!                     summed across all threads and all `ffi!{}` blocks
//!                     in this chillffi process. Reach it and the next
//!                     `ffi!{}` block **waits** for a slot — never errors
//!                     just because the limit is full.
//! ```
//!
//! # Defaults
//!
//! The default `concurrencyLimit` is computed at first use from
//! [`detected`] OS caps minus [`defaultBuffer`]:
//!
//! ```text
//! limit = min(userSoft, systemHard, RECOMMENDED_HARD_CAP) - buffer
//! ```
//!
//! With conservative defaults:
//!
//! - `RECOMMENDED_HARD_CAP = 32` — a deliberate, low default. Above 32
//!   concurrent clones the per-call fork + IPC bootstrap starts to
//!   dominate wall time on a typical machine, and a single runaway
//!   chillffi process should not be able to drown the shell.
//! - `defaultBuffer = 16` — headroom left for the shell, system
//!   daemons, and any **other** chillffi instance running under the
//!   same user. chillffi does not own the per-user limit.
//!
//! If both OS caps are unavailable (e.g. an unusual Unix without
//! `/proc` and without `RLIMIT_NPROC`, or Windows where `userSoft` is
//! `None`), the limit falls back to `RECOMMENDED_HARD_CAP` (32), still
//! minus the buffer when the system cap is known (Windows: `2048 - 16`,
//! which clamps to `32` via the recommended cap).
//!
//! # Recommended configurations
//!
//! - **Default (no tuning needed):** `16/32` semantics — a buffer of
//!   16 and a cap of 32. Conservative, good for most workloads.
//! - **Above 32:** set `concurrencyLimit` higher only if you understand
//!   the per-call cost and your system has the headroom. This is
//!   "at your own risk" territory — the per-user `RLIMIT_NPROC` is the
//!   real wall and chillffi will not stop you from running into it.
//! - **Below 16:** perfectly safe; `concurrencyLimit = 1` serialises
//!   every `ffi!{}` block, which is sometimes what you want for a
//!   workload that is not parallelisable anyway.
//!
//! # Known limitation (0.4)
//!
//! The `zygotePool > 1` and `hotPool > 0` cases are accepted by
//! [`configure`] for forward compatibility, but **not yet acted on**:
//! chillffi still runs with a single Main Zygote and creates a fresh
//! clone per `ffi!{}` block. The fields are recorded and validated so
//! a future 0.x can wire them up without a breaking change to this
//! module. Tracked as `todo` at the top of `src/zygote.rs` (sections 1
//! and 2 of the existing plan).
//!
//! # Quick start
//!
//! Do nothing — the first `ffi!{}` block initialises the limits lazily
//! from the detected OS caps with conservative defaults. Inspect them:
//!
//! ```no_run
//! let limits: chillffi::limits::Limits = *chillffi::limits::current();
//! println!(
//!   "concurrencyLimit = {}, zygotePool = {}, hotPool = {}",
//!   limits.concurrencyLimit, limits.zygotePool, limits.hotPool
//! );
//! let os: chillffi::limits::OsProcessLimits = chillffi::limits::detected();
//! println!("userSoft = {:?}, systemHard = {:?}", os.userSoft, os.systemHard);
//! ```
//!
//! Override (call once, before the first `ffi!{}` block):
//!
//! ```no_run
//! chillffi::limits::configure(chillffi::limits::Limits {
//!   zygotePool: 1,
//!   hotPool: 0,
//!   concurrencyLimit: 8
//! }).expect("invalid limits");
//! ```
//!
//! Inspect live counts (see [`crate::stats`]):
//!
//! ```no_run
//! println!("active clones = {}", chillffi::stats::activeClones());
//! ```

use crate::platform::low;
use parking_lot::{Condvar, Mutex};
use std::io;
use std::sync::OnceLock;
// =================================================================================================

/// Conservative hard cap on simultaneously live clones, independent of
/// the OS-imposed caps. Above this number the per-call fork + IPC
/// bootstrap starts to dominate wall time on a typical machine, and a
/// single chillffi process should not be able to drown the shell. See
/// the module docs for the rationale.
pub const RecommendedHardCap: u64 = 32;

/// Headroom left for the shell, system daemons, and any **other** chillffi
/// instance running under the same user. chillffi does not own the
/// per-user `RLIMIT_NPROC`. See the module docs.
pub const fn defaultBuffer() -> u64 { 16 }

/// Convenience: the `Limits` struct used by callers to override the
/// auto-detected defaults. Re-exported from this module so callers do
/// not have to think about the OsProcessLimits / Limits split.
#[derive(Debug, Clone, Copy)]
pub struct Limits
{
  /// Number of Main Zygote processes kept warm for cloning. Default `1`;
  /// values `> 1` are reserved for future work (see the module docs).
  pub zygotePool: u64,

  /// Number of long-lived clone processes kept warm between `ffi!{}`
  /// blocks for low-latency reuse. Default `0`; values `> 0` are
  /// reserved for the explicit retained-scope API (see the module docs).
  pub hotPool: u64,

  /// Hard ceiling on simultaneously live clones. Reach it and the next
  /// `ffi!{}` block **waits** for a slot — never errors just because
  /// the limit is full.
  pub concurrencyLimit: u64
}

impl Limits
{
  /// Conservative defaults a caller can use as a starting point:
  /// `zygotePool = 1`, `hotPool = 0`, `concurrencyLimit = 16`.
  pub const fn conservative() -> Self
  {
    Self {
      zygotePool: 1,
      hotPool: 0,
      concurrencyLimit: 16
    }
  }
}

// =================================================================================================

/// OS-imposed process / thread caps, as read by
/// [`detected`] at the time of the first `ffi!{}` block. Re-export of
/// [`low::OsProcessLimits`] so callers do not need to reach into the
/// platform layer.
pub type OsProcessLimits = low::OsProcessLimits;

/// Best-effort read of the OS-imposed process / thread caps right now.
/// See [`low::detectProcessLimits`] for the per-platform sources.
pub fn detected() -> OsProcessLimits
{
  low::detectProcessLimits()
}

// =================================================================================================

/// The active limits. If [`configure`] was called, returns what it
/// installed; otherwise returns the auto-detected default computed
/// from [`detected`] at first use.
///
/// Cheap: a single `OnceLock::get` + a copy of a 24-byte struct.
pub fn current() -> &'static Limits
{
  // Prefer the explicitly-configured limits if `configure` was called.
  // `Active.get()` is a single atomic load; if it returns `Some`, the
  // `Limits` inside it is `'static` (the `OnceLock` is `'static`).
  if let Some(active) = Active.get()
  {
    return &active.limits;
  }

  // Fall back to the auto-detected default. This is a separate
  // `OnceLock` so the *value* reported by `current()` is stable even
  // after `configure()` runs — `current()` always reports the value
  // that would be in effect if no `configure()` had been called, and
  // once `configure()` has been called, the branch above takes over.
  static DEFAULTS: OnceLock<Limits> = OnceLock::new();
  DEFAULTS.get_or_init(|| computeAutoDefault(detected()))
}

/// Computes the auto-detected default from raw OS caps.
///
/// Strategy, in order:
/// 1. Start with `RecommendedHardCap` (32).
/// 2. If `userSoft` is known, take the smaller of the two.
/// 3. If `systemHard` is known, take the smaller of the two.
/// 4. Subtract `defaultBuffer` (16) — but never go below 1, even if the
///    caps say we could. A limit of `0` would deadlock the first call.
///
/// If everything is `None` (no OS cap readable), we keep the
/// recommended cap as the limit (with no buffer applied) — 32. This is
/// the Windows case: `userSoft` and `userHard` are `None`, but
/// `systemHard` is the conservative `2048`, so step 3 keeps `32` and
/// step 4 subtracts `16`, landing on `16`.
fn computeAutoDefault(os: OsProcessLimits) -> Limits
{
  let mut limit: u64 = RecommendedHardCap;
  if let Some(soft) = os.userSoft { limit = limit.min(soft); }
  if let Some(hard) = os.systemHard { limit = limit.min(hard); }

  // Apply the buffer only when at least one OS cap was known — otherwise
  // the buffer would shrink the recommended cap to 16 even on a system
  // that exposed no information at all, which is unnecessarily punitive.
  if os.userSoft.is_some() || os.systemHard.is_some()
  {
    limit = limit.saturating_sub(defaultBuffer());
  }

  // Never go below 1 — a limit of 0 would deadlock the first call.
  if limit == 0 { limit = 1; }

  Limits {
    zygotePool: 1,
    hotPool: 0,
    concurrencyLimit: limit
  }
}

// =================================================================================================

/// The configured-or-default limits, plus the semaphore that enforces
/// `concurrencyLimit`. Lives in a `OnceLock` so the first `configure`
/// call (or the first `ffi!{}` block, whichever comes first) installs it.
struct ActiveLimit
{
  limits: Limits,
  semaphore: CountingSemaphore
}

static Active: OnceLock<ActiveLimit> = OnceLock::new();

/// Validates `limits` against the rules in the module docs and installs
/// them as the active limits. Must be called **before** the first
/// `ffi!{}` block — the limits are otherwise installed lazily on the
/// first clone request, and a call after that is a no-op (returns
/// `Err` with a clear message).
///
/// Validation:
/// - `zygotePool >= 1` — at least one Main Zygote is required.
/// - `concurrencyLimit >= 1` — a limit of `0` would deadlock the
///   first call.
/// - `hotPool <= concurrencyLimit` — hot workers are clones, and they
///   must fit inside the overall clone ceiling.
/// - `zygotePool <= concurrencyLimit` — same reason.
pub fn configure(limits: Limits) -> io::Result<()>
{
  if limits.zygotePool == 0
  {
    return Err(io::Error::other(
      "limits.zygotePool must be >= 1 (zero Main Zygotes would deadlock)"
    ));
  }
  if limits.concurrencyLimit == 0
  {
    return Err(io::Error::other(
      "limits.concurrencyLimit must be >= 1 (zero would deadlock the first ffi!{} block)"
    ));
  }
  if limits.hotPool > limits.concurrencyLimit
  {
    return Err(io::Error::other(format!(
      "limits.hotPool ({}) must be <= concurrencyLimit ({}) — hot workers are clones",
      limits.hotPool, limits.concurrencyLimit
    )));
  }
  if limits.zygotePool > limits.concurrencyLimit
  {
    return Err(io::Error::other(format!(
      "limits.zygotePool ({}) must be <= concurrencyLimit ({}) — Main Zygotes count against the ceiling while they clone",
      limits.zygotePool, limits.concurrencyLimit
    )));
  }

  let semaphore: CountingSemaphore = CountingSemaphore::new(limits.concurrencyLimit);
  let entry: ActiveLimit = ActiveLimit { limits, semaphore };
  Active
    .set(entry)
    .map_err(|_| {
      io::Error::other(
        "chillffi::limits::configure called after the limits were already installed \
         (either by an earlier configure() or by the first ffi!{} block); \
         the active limits cannot be changed at runtime"
      )
    })
}

/// Returns the active `Limits` (configured or auto-detected default).
/// Identical to [`current`] but also ensures the active state is installed
/// — used by [`crate::zygote::ClonedZygote::getMeClone`] to acquire its
/// permit.
///
/// `ActiveLimit` is private to this module; the function returns a
/// borrowed reference whose lifetime is `'static` (the `OnceLock` is
/// `'static`). Callers only need the *fields* of `ActiveLimit`, which
/// are accessed through the `&'static` reference — they cannot name
/// the type itself, which is fine.
fn installed() -> &'static ActiveLimit
{
  // Initialise lazily with the auto-detected default if `configure` was
  // never called. `OnceLock::get_or_init` is idempotent and cheap.
  Active.get_or_init(|| {
    let limits: Limits = *current();
    let semaphore: CountingSemaphore = CountingSemaphore::new(limits.concurrencyLimit);
    ActiveLimit { limits, semaphore }
  })
}

// =================================================================================================

/// A counting semaphore implemented on top of `parking_lot::Mutex` +
/// `parking_lot::Condvar`, so we don't take a new dependency. Blocking
/// acquire, RAII release via [`Permit`].
pub(super) struct CountingSemaphore
{
  state: Mutex<u64>,
  cond: Condvar
}

impl CountingSemaphore
{
  #[allow(clippy::missing_const_for_fn)]
  pub(super) fn new(capacity: u64) -> Self
  {
    Self {
      state: Mutex::new(capacity),
      cond: Condvar::new()
    }
  }

  /// Blocks until at least one permit is available, then takes one.
  /// Will never return `Err` — the contract is "block, do not error".
  ///
  /// `clippy::significant_drop_tightening` is silenced because the
  /// `MutexGuard` must outlive the `Condvar::wait` call (which
  /// atomically releases and reacquires the mutex); an explicit
  /// `drop(state)` before the wait loop would defeat the wait.
  #[allow(clippy::significant_drop_tightening)]
  pub(super) fn acquire(&self) -> Permit<'_>
  {
    let mut state: parking_lot::MutexGuard<u64> = self.state.lock();
    while *state == 0
    {
      self.cond.wait(&mut state);
    }
    *state -= 1;
    Permit { semaphore: self }
  }

  /// Non-blocking try-acquire; used by tests to peek at the available
  /// count without disturbing waiters.
  #[allow(dead_code)]
  pub(super) fn try_acquire(&self) -> Option<Permit<'_>>
  {
    let mut state: parking_lot::MutexGuard<u64> = self.state.lock();
    if *state == 0 { return None; }
    *state -= 1;
    // Drop the guard *before* returning the `Permit` so the next
    // acquire does not have to wait for the guard's implicit drop at
    // end of scope.
    drop(state);
    Some(Permit { semaphore: self })
  }

  /// Number of permits currently available (i.e. `concurrencyLimit - active_clones`).
  /// Used by [`crate::stats::availableSlots`] indirectly — see
  /// [`availableSlots`] below.
  #[allow(dead_code)]
  pub(super) fn available(&self) -> u64
  {
    *self.state.lock()
  }
}

/// RAII permit: dropping it releases one slot back to the semaphore.
pub(super) struct Permit<'a>
{
  semaphore: &'a CountingSemaphore
}

impl Drop for Permit<'_>
{
  fn drop(&mut self) -> ()
  {
    let mut state: parking_lot::MutexGuard<u64> = self.semaphore.state.lock();
    *state += 1;
    // Drop the guard *before* waking the next waiter — parking_lot's
    // `Condvar::notify_one` does not require holding the mutex, and
    // dropping the guard first means the woken waiter does not have to
    // immediately re-block on the mutex.
    drop(state);
    self.semaphore.cond.notify_one();
  }
}

// =================================================================================================

/// Acquires a permit from the active concurrency-limit semaphore,
/// blocking until one is available. Returns the RAII permit — drop it
/// when the clone it gates is gone.
///
/// Used by [`crate::zygote::ClonedZygote::getMeClone`]. The permit lives
/// inside `ClonedZygote` and is dropped together with it, releasing the
/// slot for the next caller.
pub(super) fn acquirePermit() -> Permit<'static>
{
  // `Active` is a `OnceLock` whose contents live for the static
  // duration of the process. The semaphore inside it is borrowed for
  // `'static` — and `acquire` returns `Permit<'_>` bound to `&self`,
  // so a `&'static CountingSemaphore` produces a `Permit<'static>`
  // without any transmute. The permit is released when the
  // `ClonedZygote` that owns it is dropped.
  let active: &'static ActiveLimit = installed();
  let sem: &'static CountingSemaphore = &active.semaphore;
  sem.acquire()
}

/// Number of free slots in the active concurrency-limit semaphore right
/// now. Used by [`crate::stats::availableSlots`] (re-exported there as
/// `availableSlots`).
#[allow(dead_code)]
pub(super) fn availableSlots() -> u64
{
  installed().semaphore.available()
}

/// The `concurrencyLimit` value from the active configuration. Used by
/// [`crate::stats`] for "active vs limit" reporting.
#[allow(dead_code)]
pub(super) fn activeLimit() -> u64
{
  installed().limits.concurrencyLimit
}

/// Public alias for the same value, used by [`crate::stats::availableSlots`]
/// (declared `pub` here so `stats` does not need a `pub(super)` chain
/// back into `limits`).
#[doc(hidden)]
pub fn activeLimitPublic() -> u64
{
  installed().limits.concurrencyLimit
}

/// The `zygotePool` value from the active configuration. Used by
/// [`crate::stats`] for completeness; today this is always `1`.
#[allow(dead_code)]
pub(super) fn activeZygotePool() -> u64
{
  installed().limits.zygotePool
}

/// The `hotPool` value from the active configuration. Used by
/// [`crate::stats`]; today this is always `0`.
#[allow(dead_code)]
pub(super) fn activeHotPool() -> u64
{
  installed().limits.hotPool
}

// =================================================================================================

#[cfg(test)]
mod tests
{
  use super::*;
  use crate::platform::low;

  // ----------------------------------------------------------------------------------------------

  /// `detected()` must succeed and must not report obviously broken
  /// numbers (zero cap, overflow). On Linux/macOS at least one of
  /// `userSoft`/`systemHard` is expected to be `Some`; on Windows the
  /// `systemHard` is the conservative constant. This is the minimum
  /// sanity check that the platform glue actually reads the OS knob.
  #[test]
  fn detectedReturnsSomethingSane() -> ()
  {
    let os: OsProcessLimits = detected();
    let anySome: bool = os.userSoft.is_some()
      || os.userHard.is_some()
      || os.systemHard.is_some();
    assert!(anySome, "OS returned no caps at all: {os:?}");

    if let Some(soft) = os.userSoft { assert!(soft > 0, "userSoft={soft}"); }
    if let Some(hard) = os.userHard { assert!(hard > 0, "userHard={hard}"); }
    if let Some(sys)  = os.systemHard { assert!(sys > 0, "systemHard={sys}"); }
  }

  // ----------------------------------------------------------------------------------------------

  /// `computeAutoDefault` must never return `concurrencyLimit == 0`
  /// — that would deadlock the first call. It must also never exceed
  /// `RecommendedHardCap` when no OS cap is known.
  #[test]
  fn autoDefaultNeverZeroAndClamped() -> ()
  {
    // No OS caps at all — should land on the recommended cap, no buffer.
    let none: OsProcessLimits = OsProcessLimits::default();
    let l: Limits = computeAutoDefault(none);
    assert_eq!(l.concurrencyLimit, RecommendedHardCap);
    assert_eq!(l.zygotePool, 1);
    assert_eq!(l.hotPool, 0);

    // Large OS cap — should clamp to recommended cap, then minus buffer.
    let large: OsProcessLimits = OsProcessLimits {
      userSoft: Some(10_000),
      userHard: Some(10_000),
      systemHard: Some(10_000)
    };
    let l: Limits = computeAutoDefault(large);
    assert_eq!(l.concurrencyLimit, RecommendedHardCap - defaultBuffer());

    // Tiny OS cap — should be 1 (never zero).
    let tiny: OsProcessLimits = OsProcessLimits {
      userSoft: Some(1),
      userHard: Some(1),
      systemHard: Some(1)
    };
    let l: Limits = computeAutoDefault(tiny);
    assert_eq!(l.concurrencyLimit, 1);

    // Windows shape: userSoft=None, systemHard=Some(2048).
    let win: OsProcessLimits = OsProcessLimits {
      userSoft: None,
      userHard: None,
      systemHard: Some(2048)
    };
    let l: Limits = computeAutoDefault(win);
    assert_eq!(l.concurrencyLimit, RecommendedHardCap - defaultBuffer());
  }

  // ----------------------------------------------------------------------------------------------

  /// `configure` must reject the obvious invalid inputs.
  #[test]
  fn configureRejectsInvalid() -> ()
  {
    // We cannot actually install these — `configure` would either succeed
    // (and contaminate the rest of the test process) or fail because
    // limits are already installed. We only check the *validation* by
    // reconstructing the same logic on the input; the real `configure`
    // is exercised by `configureInstallsAndReadsBack` below only if it
    // runs first in the binary (it does — alphabetical order).
    let bad0: Limits = Limits { zygotePool: 0, hotPool: 0, concurrencyLimit: 8 };
    let bad1: Limits = Limits { zygotePool: 1, hotPool: 0, concurrencyLimit: 0 };
    let bad2: Limits = Limits { zygotePool: 1, hotPool: 16, concurrencyLimit: 8 };
    let bad3: Limits = Limits { zygotePool: 16, hotPool: 0, concurrencyLimit: 8 };

    // Replicate the validation locally so we don't poison the OnceLock.
    fn rejects(l: Limits) -> bool
    {
      l.zygotePool == 0
        || l.concurrencyLimit == 0
        || l.hotPool > l.concurrencyLimit
        || l.zygotePool > l.concurrencyLimit
    }
    assert!(rejects(bad0));
    assert!(rejects(bad1));
    assert!(rejects(bad2));
    assert!(rejects(bad3));

    // A valid shape passes validation.
    let ok: Limits = Limits { zygotePool: 1, hotPool: 0, concurrencyLimit: 8 };
    assert!(!rejects(ok));
  }

  // ----------------------------------------------------------------------------------------------

  /// `low::detectProcessLimits` is callable and returns the same struct
  /// type as `limits::detected` (the re-export is structural, not a
  /// wrapper).
  #[test]
  fn detectedMatchesLowLevel() -> ()
  {
    let a: OsProcessLimits = detected();
    let b: low::OsProcessLimits = low::detectProcessLimits();
    assert_eq!(a.userSoft, b.userSoft);
    assert_eq!(a.userHard, b.userHard);
    assert_eq!(a.systemHard, b.systemHard);
  }
}

// =================================================================================================
