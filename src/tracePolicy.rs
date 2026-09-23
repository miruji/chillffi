//! Global default controlling whether `chillffi` prints a trace of every
//! FFI request and response.
//!
//! Trace is the debug channel that the rest of `chillffi` deliberately avoids
//! having while running: every FFI call happens in a separate forked process,
//! so without trace a crash or a hang in the C side surfaces as just
//! `FFIError::ZygoteCommunicationFailed("…")` — a single opaque line. Trace
//! turns that into a step-by-step log of what was sent, what the clone did
//! with it, and what came back.
//!
//! # Priority (most specific wins)
//!
//! 1. per-call override — [`crate::ffi::library::CallBuilder::trace`] /
//!    [`crate::ffi::library::CallBuilder::noTrace`]
//! 2. scope-level override — [`crate::ffi::scope::Scope::setTrace`]
//! 3. this global default — [`setGlobalTrace`]
//! 4. the `ChillffiTrace` environment variable, read once on first use
//!
//! Per-call and scope overrides affect only the **Runtime side** of the trace
//! (what the parent process sends / receives). The **clone side** (what the
//! forked worker actually did with the request) is governed by the
//! `ChillffiTrace` environment variable alone — because clones inherit their
//! environment from `fork` (Linux/macOS) or `spawn` (Windows), an env var set
//! before the program starts reaches every clone for free, with no IPC
//! protocol change.
//!
//! # Environment variable format
//!
//! `ChillffiTrace` is parsed case-insensitively:
//! - `1`, `true`, `yes`, `on` → trace enabled
//! - `0`, `false`, `no`, `off`, unset → trace disabled
//! - anything else → disabled (conservative default)
//!
//! # Example
//!
//! ```no_run
//! use chillffi::tracePolicy::setGlobalTrace;
//! use chillffi::ffi;
//!
//! // Turn trace on programmatically (Runtime side only — for the clone side
//! // use `ChillffiTrace=1` before starting the program).
//! setGlobalTrace(true);
//!
//! let result: f64 = ffi!(|scope| {
//!   let libm: Library = scope.load("libm.so.6")?;
//!   libm.call("sqrt").arg::<f64>(4.0).result()
//! }).expect("sqrt failed");
//! # let _ = result;
//! ```
// =================================================================================================
use std::sync::atomic::{AtomicBool, Ordering};
// =================================================================================================

/// Name of the environment variable consulted once on first use of
/// [`globalTrace`] to seed the global default.
///
/// Clones inherit this variable from `fork` / `spawn`, so a single setting
/// reaches both sides of the IPC boundary without any protocol change.
pub const TraceEnvVar: &str = "ChillffiTrace";

/// Global default: starts `false`, but the first call to [`globalTrace`]
/// seeds it from `ChillffiTrace` if that env var is set.
static GlobalTrace: AtomicBool = AtomicBool::new(false);

/// Once-set guard: makes the env-var read happen exactly once per process,
/// on the first [`globalTrace`] call. Subsequent [`setGlobalTrace`] calls
/// still flip the atomic — the env var is only the *initial* default.
static EnvSeeded: AtomicBool = AtomicBool::new(false);

// =================================================================================================

/// Parses the `ChillffiTrace` environment variable value.
///
/// Recognized truthy values: `1`, `true`, `yes`, `on` (case-insensitive).
/// Anything else — including the variable being absent — is treated as
/// "off", so a stray `ChillffiTrace=garbage` cannot accidentally enable a
/// debug firehose.
fn parseTraceEnv(value: Option<std::ffi::OsString>) -> bool
{
  // Lowercase ASCII comparison keeps this allocation-free and dependency-free.
  let Some(value) = value else { return false; };
  let value: String = value.to_string_lossy().into_owned().to_ascii_lowercase();
  matches!(value.as_str(), "1" | "true" | "yes" | "on")
}

/// Seeds [`GlobalTrace`] from `ChillffiTrace` exactly once per process.
///
/// Idempotent — concurrent callers race to set `EnvSeeded`, the loser's
/// `store` is a no-op because `EnvSeeded` was already `true`. The env read
/// itself is cheap and side-effect-free, so racing it is fine.
fn seedFromEnvOnce() -> ()
{
  if EnvSeeded.swap(true, Ordering::AcqRel) {
    return; // Already seeded by someone else
  }
  let enabled: bool = parseTraceEnv(std::env::var_os(TraceEnvVar));
  GlobalTrace.store(enabled, Ordering::Relaxed);
}

/// Sets the global default for trace output. Affects only calls that don't
/// specify their own scope or per-call override.
///
/// Does **not** retroactively turn on trace inside already-forked clones —
/// they captured their env at `fork`/`spawn` time. To trace the clone side
/// too, set `ChillffiTrace=1` in the parent's environment *before* the
/// program starts.
pub fn setGlobalTrace(enabled: bool) -> ()
{
  // Make sure the env-seed step has run, so a later `setGlobalTrace(false)`
  // actually means "off" rather than "still waiting to be seeded from env".
  seedFromEnvOnce();
  GlobalTrace.store(enabled, Ordering::Relaxed);
}

/// Reads the current global default for trace output.
///
/// First call seeds the value from `ChillffiTrace` if set; subsequent calls
/// return whatever [`setGlobalTrace`] last stored, or the seeded default.
pub fn globalTrace() -> bool
{
  seedFromEnvOnce();
  GlobalTrace.load(Ordering::Relaxed)
}

/// Reads `ChillffiTrace` directly, without touching the global atomic.
///
/// Used by the clone-side trace hook in [`crate::worker`] so a freshly forked
/// clone can decide whether to log at the IPC boundary, independent of any
/// [`setGlobalTrace`] calls the parent may have made after forking it.
pub fn envTrace() -> bool
{
  parseTraceEnv(std::env::var_os(TraceEnvVar))
}

// =================================================================================================

#[cfg(test)]
mod tests
{
  use super::*;
  use std::sync::Mutex;
  // ===============================================================================================

  // `setGlobalTrace` mutates a process-wide static, so the tests serialize
  // through this mutex — otherwise parallel `cargo test` threads would race
  // on the same atomic and stomp each other's expected values.
  static TraceTestLock: Mutex<()> = Mutex::new(());

  // ===============================================================================================

  /// Checks the recognized truthy values of `ChillffiTrace`.
  #[test]
  fn envParsingRecognizesTruthy() -> ()
  {
    for value in ["1", "true", "TRUE", "True", "yes", "Yes", "on", "ON"]
    {
      assert!(
        parseTraceEnv(Some(value.into())),
        "expected `{value}` to be parsed as truthy"
      );
    }
  }

  /// Checks the recognized falsy / unrecognized values.
  #[test]
  fn envParsingRecognizesFalsy() -> ()
  {
    for value in ["0", "false", "no", "off", "", "garbage", "2", "yep"]
    {
      assert!(
        !parseTraceEnv(Some(value.into())),
        "expected `{value}` to be parsed as falsy"
      );
    }
    assert!(!parseTraceEnv(None), "absent env var should mean off");
  }

  /// `setGlobalTrace` round-trips through the atomic.
  #[test]
  fn globalRoundtrip() -> ()
  {
    let _guard: std::sync::MutexGuard<()> = TraceTestLock.lock().unwrap();
    setGlobalTrace(true);
    assert!(globalTrace());
    setGlobalTrace(false);
    assert!(!globalTrace());
  }

  // ===============================================================================================
}

// =================================================================================================