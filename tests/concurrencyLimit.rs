//! Integration test for the concurrency limit — runs in a **fresh process**
//! (integration tests are separate binaries) so [`chillffi::limits::configure`]
//! can install a small `concurrencyLimit` without contending with the rest
//! of the test suite.
//!
//! The test suite in this file is a single `#[test]` function so its
//! steps run in a fixed, deterministic order — the three phases
//! (install, workload, post-install-rejection) cannot be parallelised
//! by `cargo test`, which is exactly the guarantee we need (the
//! `OnceLock` behind `limits::configure` is one-shot).

#[path = "../examples/platform/mod.rs"]
mod platform;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

use chillffi::ffi;
use chillffi::limits;
use chillffi::stats;
use platform::LibmPath;

#[test]
fn concurrencyLimitLifecycle() -> ()
{
  // ============================================================
  // Phase 1: invalid shapes must be rejected BEFORE the OnceLock is
  // touched. These calls never install anything — they fail validation
  // first. Run them on a fresh process to be sure no limits are
  // installed yet.
  // ============================================================
  let invalidShapes: Vec<limits::Limits> = vec![
    limits::Limits { zygotePool: 0, hotPool: 0, concurrencyLimit: 4 },
    limits::Limits { zygotePool: 1, hotPool: 0, concurrencyLimit: 0 },
    limits::Limits { zygotePool: 1, hotPool: 8, concurrencyLimit: 4 },
    limits::Limits { zygotePool: 8, hotPool: 0, concurrencyLimit: 4 }
  ];
  for limits in &invalidShapes
  {
    let result: std::io::Result<()> = limits::configure(*limits);
    assert!(
      result.is_err(),
      "configure should have rejected invalid limits: {limits:?}"
    );
  }

  // ============================================================
  // Phase 2: install a tiny limit. configure() can only run once
  // per process, so this is the only place where we are allowed to
  // set it. After this point, limits::current() must reflect what we
  // just installed.
  // ============================================================
  limits::configure(limits::Limits {
    zygotePool: 1,
    hotPool: 0,
    concurrencyLimit: 2
  }).expect("configure(2) must succeed on a fresh process");

  assert_eq!(limits::current().concurrencyLimit, 2);
  assert_eq!(limits::current().zygotePool, 1);
  assert_eq!(limits::current().hotPool, 0);

  // ============================================================
  // Phase 3: a parallel workload under the small limit must
  // succeed (block, not error) and must never exceed the limit.
  // ============================================================
  let peak: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
  const Threads: usize = 8;
  const PerThread: usize = 20;

  let handles: Vec<thread::JoinHandle<u32>> = (0..Threads)
    .map(|_t| {
      let peak = Arc::clone(&peak);
      thread::spawn(move || -> u32 {
        let mut localErrors: u32 = 0;
        for _ in 0..PerThread
        {
          let result: Result<f64, _> = ffi!(|scope| {
            // Inside the clone: this worker holds a slot. Bump the
            // shared peak so we can assert it stayed <= limit.
            let active: u64 = stats::activeClones();
            let mut current: u64 = peak.load(Ordering::Acquire);
            while active > current
            {
              match peak.compare_exchange_weak(
                current,
                active,
                Ordering::AcqRel,
                Ordering::Acquire
              ) {
                Ok(_) => break,
                Err(observed) => current = observed
              }
            }

            let libm: Library = scope.load(LibmPath)?;
            libm.call("sqrt").arg::<f64>(4.0).result()
          });

          match result {
            Ok(value) => {
              assert!(
                (value - 2.0).abs() < f64::EPSILON,
                "sqrt(4.0) returned {value}, expected 2.0"
              );
            }
            Err(e) => {
              eprintln!("[test] ffi!{{}} failed: {e}");
              localErrors += 1;
            }
          }
        }
        localErrors
      })
    })
    .collect();

  // IMPORTANT: collect first, then join — collecting the
  // `JoinHandle`s is what starts the threads in parallel. If we
  // iterated directly (clippy's `needless_collect` suggestion), each
  // spawn would be followed by a join before the next spawn — which
  // would not stress the concurrency limit at all and the test would
  // be vacuous.
  let mut totalErrors: u32 = 0;
  for handle in handles
  {
    totalErrors += handle.join().expect("worker thread panicked");
  }

  assert_eq!(
    totalErrors, 0,
    "no ffi!{{}} block may error just because the concurrency limit is full"
  );

  let peakValue: u64 = peak.load(Ordering::Acquire);
  assert!(
    peakValue <= 2,
    "peak activeClones ({peakValue}) exceeded the configured limit (2) — \
     the semaphore is not actually gating getMeClone()"
  );
  assert!(
    peakValue >= 1,
    "peak activeClones ({peakValue}) stayed at 0 — the test never observed \
     an in-flight clone, which means the test machinery is broken"
  );

  // Final stats sanity: no clones in flight after everyone joins.
  let active: u64 = stats::activeClones();
  assert_eq!(active, 0, "after all workers join, no clones may be in flight");
  assert!(stats::zygotesAlive() >= 1, "Main Zygote must still be alive");
  assert_eq!(stats::hotAlive(), 0, "no hot pool today");
  assert_eq!(
    stats::availableSlots(),
    stats::configured().concurrencyLimit,
    "availableSlots must equal concurrencyLimit when no clones are in flight"
  );

  // ============================================================
  // Phase 4: configure() called a second time must error with a clear
  // "already installed" message. Runtime reconfiguration is not
  // supported (it would race with active clones).
  // ============================================================
  let result: std::io::Result<()> = limits::configure(limits::Limits {
    zygotePool: 1,
    hotPool: 0,
    concurrencyLimit: 4
  });
  let err = result.expect_err(
    "configure() after install must error, not silently no-op"
  );
  assert!(
    err.to_string().contains("already installed"),
    "error message should mention \"already installed\", got: {err}"
  );
}