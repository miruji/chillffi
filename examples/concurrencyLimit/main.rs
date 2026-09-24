//! Example: live process-pool monitoring with `chillffi::stats` and
//! `chillffi::limits`.
//!
//! What it shows:
//! - Override the auto-detected default by calling `limits::configure`
//!   before the first `ffi!{}` block.
//! - Spin up 6 worker threads that each make a few `ffi!{}` calls.
//! - Run a dashboard thread that prints the live counters every 50 ms
//!   while the workers are busy.
//! - At the end, print the final limits + counters so the user can
//!   see what chillffi auto-detected on their machine.
//!
//! Run with:
//! ```bash
//! cargo run --example concurrencyLimit
//! ```
// =================================================================================================
#[path = "../platform/mod.rs"]
mod platform;
use platform::LibmPath;
// =================================================================================================
use chillffi::ffi;
use chillffi::limits;
use chillffi::stats;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
// =================================================================================================

fn main() -> ()
{
  // -----------------------------------------------------------------------------------------------
  // 1. Inspect what chillffi auto-detected from the OS.
  
  let os: limits::OsProcessLimits = limits::detected();
  println!("OS-detected process caps:");
  println!("  userSoft   = {:?}", os.userSoft);
  println!("  userHard   = {:?}", os.userHard);
  println!("  systemHard = {:?}", os.systemHard);
  println!();

  // -----------------------------------------------------------------------------------------------
  // 2. Override the defaults BEFORE the first ffi!{} block.
  //    configure() can only run once per process; the first ffi!{} block
  //    triggers the auto-detected default if you don't call it first.
  //
  //    Here we pick a small limit so the dashboard is interesting
  //    (workers will block on the semaphore and you'll see
  //    activeClones stay at the cap).
  
  limits::configure(limits::Limits {
    zygotePool: 1,
    hotPool: 0,
    concurrencyLimit: 3
  }).expect("configure(3) must succeed on a fresh process");

  let active: limits::Limits = *limits::current();
  println!("Active limits (configured):");
  println!("  zygotePool       = {}", active.zygotePool);
  println!("  hotPool          = {}", active.hotPool);
  println!("  concurrencyLimit = {}", active.concurrencyLimit);
  println!();

  // -----------------------------------------------------------------------------------------------
  // 3. Start a dashboard thread that prints live counters every 50 ms
  //    while workers are running. Stop flag is set when main is done.
  
  let stop: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
  let stopDash = Arc::clone(&stop);
  let dashboard: thread::JoinHandle<()> = thread::spawn(move || {
    println!("Dashboard (activeClones / availableSlots / zygotesAlive / hotAlive):");
    while !stopDash.load(Ordering::Acquire)
    {
      println!(
        "  clones={:>2}/{}  free={:<2}  zygotes={}  hot={}",
        stats::activeClones(),
        stats::configured().concurrencyLimit,
        stats::availableSlots(),
        stats::zygotesAlive(),
        stats::hotAlive()
      );
      thread::sleep(Duration::from_millis(50));
    }
  });

  // -----------------------------------------------------------------------------------------------
  // 4. Spawn 6 worker threads, each doing 4 sqrt() calls. With
  //    concurrencyLimit=3 and 6 threads you should see `clones` reach
  //    the cap of 3, and the workers that don't fit will block (not
  //    error) until a slot frees.
  //
  //    Each ffi!{} block also sleeps 100 ms (Rust-side, inside the
  //    closure that runs in the Runtime) — long enough that the
  //    dashboard thread (which samples every 50 ms) catches the cap
  //    of 3 in flight several times. The clone is alive for the
  //    whole duration of the closure, so `activeClones` reflects it.
  
  const Threads: usize = 6;
  const PerThread: usize = 4;
  let start: Instant = Instant::now();
  let handles: Vec<thread::JoinHandle<()>> = (0..Threads)
    .map(|t| thread::spawn(move || {
      for i in 0..PerThread
      {
        // sqrt(x^2) == x for x >= 0 — pick x = (i + 1) so the input
        // value varies across iterations while staying easy to check.
        let input: f64 = (i + 1) as f64;
        let expected: f64 = input;
        let value: f64 = ffi!(|scope| {
          let libm: Library = scope.load(LibmPath)?;
          // Hold the slot for 100 ms so the dashboard can see
          // `activeClones` sit at the cap (3 of 3).
          thread::sleep(Duration::from_millis(100));
          libm.call("sqrt").arg::<f64>(input * input).result()
        })
        .unwrap_or_else(|e| panic!("worker {t} iteration {i} failed: {e}"));
        assert!(
          (value - expected).abs() < f64::EPSILON,
          "sqrt({}) = {value}, expected {expected}", input * input
        );
      }
    }))
    .collect();

  for h in handles { h.join().expect("worker thread panicked"); }

  stop.store(true, Ordering::Release);
  dashboard.join().expect("dashboard thread panicked");

  // -----------------------------------------------------------------------------------------------
  // 5. Final state.
  println!();
  println!("Done in {:?}.", start.elapsed());
  println!("Final stats:");
  println!("  activeClones   = {} (must be 0 — no ffi! in flight)", stats::activeClones());
  println!("  availableSlots = {} (must equal concurrencyLimit)", stats::availableSlots());
  println!("  zygotesAlive   = {}", stats::zygotesAlive());
  println!("  hotAlive       = {}", stats::hotAlive());
}

// =================================================================================================