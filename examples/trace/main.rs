#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::LibmPath;
// =================================================================================================
use chillffi::ffi;
use chillffi::tracePolicy::setGlobalTrace;
// =================================================================================================

/// Trace flag — the debug channel that turns an opaque
/// `FFIError::ZygoteCommunicationFailed("…")` into a step-by-step log of what
/// was sent, what the forked clone did with it, and what came back.
///
/// Four priority layers (most specific wins):
/// 1. per-call — `.trace()` / `.noTrace()` on the call builder
/// 2. scope — `Scope::setTrace(bool)`
/// 3. global — [`setGlobalTrace`]
/// 4. env var — `ChillffiTrace=1` (the only one that reaches the clone side
///    without a per-call opt-in, because clones inherit env from `fork` /
///    `spawn`)
///
/// Run with `cargo run --example trace` for the on-by-default layers, or with
/// `ChillffiTrace=1 cargo run --example trace` to also see every request's
/// clone-side log line.
fn main() -> ()
{
  perCall();
  scope();
  global();
  // envVar is exercised by `ChillffiTrace=1 cargo run --example trace`,
  // not by anything inside this binary — there's no per-process "set env var
  // in already-forked children" API, that's the whole point of the env var.
  println!("ok: set ChillffiTrace=1 before running to see every request on the clone side too");
}

// =================================================================================================

/// `.trace()` on a single call — Runtime side logs the request and response,
/// the `trace` flag rides on the request so the forked clone logs its own
/// work too. Other calls in the same scope are unaffected.
fn perCall() -> ()
{
  // First call: traced.
  let r1: f64 = ffi!(|scope| {
    let libm: Library = scope.load(LibmPath)?;
    libm.call("sqrt").arg::<f64>(16.0).trace().result()
  }).expect("traced sqrt failed");

  // Second call in a fresh scope: NOT traced (no `.trace()` on this one).
  let r2: f64 = ffi!(|scope| {
    let libm: Library = scope.load(LibmPath)?;
    libm.call("sqrt").arg::<f64>(25.0).result()
  }).expect("untraced sqrt failed");

  assert!((r1 - 4.0).abs() < f64::EPSILON);
  assert!((r2 - 5.0).abs() < f64::EPSILON);
  println!("ok: per-call trace (sqrt(16.0)={}, sqrt(25.0)={})", r1, r2);
}

/// `Scope::setTrace(true)` — every call in this block is traced. A `.noTrace()`
/// on a specific call inside still wins (most-specific-first), demonstrating
/// the override chain.
fn scope() -> ()
{
  let (a, b): (f64, f64) = ffi!(|scope| {
    scope.setTrace(true);
    let libm: Library = scope.load(LibmPath)?;

    // Traced: inherits the scope's `setTrace(true)`.
    let a: f64 = libm.call("sqrt").arg::<f64>(36.0).result()?;

    // NOT traced: `.noTrace()` overrides the scope default.
    let b: f64 = libm.call("sqrt").arg::<f64>(49.0).noTrace().result()?;
    Ok((a, b))
  }).expect("scope trace failed");

  assert!((a - 6.0).abs() < f64::EPSILON);
  assert!((b - 7.0).abs() < f64::EPSILON);
  println!("ok: scope trace (sqrt(36.0)={} traced, sqrt(49.0)={} untraced)", a, b);
}

/// `setGlobalTrace(true)` — every subsequent call (in any scope, on any
/// thread) is traced unless a scope or per-call override turns it off.
///
/// Affects only the **Runtime side** — clones are already forked by the time
/// `main` runs, so a programmatic `setGlobalTrace(true)` cannot retroactively
/// turn on clone-side logging for them. Use `ChillffiTrace=1` *before*
/// starting the program for that.
fn global() -> ()
{
  setGlobalTrace(true);
  let result: f64 = ffi!(|scope| {
    let libm: Library = scope.load(LibmPath)?;
    libm.call("sqrt").arg::<f64>(64.0).result()
  }).expect("global trace failed");
  setGlobalTrace(false);

  assert!((result - 8.0).abs() < f64::EPSILON);
  println!("ok: global trace (sqrt(64.0)={})", result);
}

// =================================================================================================
