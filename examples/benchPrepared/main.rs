#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::LibmPath;
// =================================================================================================
use chillffi::ffi;
use chillffi::ffi::library::Function;
use chillffi::ffi::scope::Scope;
use chillffi::ffi::types::Type;
use std::time::Instant;
// =================================================================================================

/// A/B benchmark for the "prepared functions" proposal.
///
/// All three loops run inside ONE `ffi!` scope — one clone, one library — so
/// the only difference between them is what the clone does per request:
///
/// 1. `Call`         — full path: `dlsym` + `ffi_prep_cif` + marshal + call
///    (this is what the regular `lib.call(...)` does today);
/// 2. `CallPrepared` — `dlsym` and `ffi_prep_cif` were done once at prepare
///    time; per call: validate + marshal + call;
/// 3. `ReadMemory`   — no libffi at all, a minimal request/response round
///    trip: the IPC floor every request pays.
///
/// The gap (1) - (2) is exactly what the proposal saves per call; the gap
/// (2) - (3) is validation + marshaling + the actual C call.
fn main() -> ()
{
  const Iterations: usize = 2000;
  const Warmup: usize = 200;
  const ScopeSamples: usize = 20;
  const Rounds: usize = 3;

  // Context: cost of one `ffi!` block with no work — fork + IPC bootstrap + kill.
  let t: Instant = Instant::now();
  for _ in 0..ScopeSamples
  {
    let _: i32 = ffi!(|_scope| { Ok(0i32) }).expect("empty scope failed");
  }
  let perScopeUs: f64 = t.elapsed().as_secs_f64() * 1e6 / ScopeSamples as f64;
  println!("empty ffi! scope (fork+bootstrap+kill): {perScopeUs:.2} us/scope");
  println!("iterations per loop: {Iterations} x {Rounds} rounds, min taken");
  println!();

  let (regularUs, preparedUs, read16Us, read0Us, _checksum): (f64, f64, f64, f64, f64) =
    ffi!(|scope| {
    let libm: Library = scope.load(LibmPath)?;
    let mem = scope.alloc(16)?;

    // --- warmup both paths (and the IPC channel) ---
    for i in 0..Warmup
    {
      let x: f64 = i as f64 + 1.0;
      let _ = libm.call("sqrt").arg::<f64>(x).result::<f64>()?;
    }
    let sqrt: Function = libm.function("sqrt", &[Type::F64], Type::F64)?;
    for i in 0..Warmup
    {
      let x: f64 = i as f64 + 1.0;
      let _ = sqrt.call().arg(x).result::<f64>()?;
    }
    for _ in 0..Warmup { let _ = mem.read()?; }

    // Per-probe minimum over interleaved rounds — cancels drift/frequency noise.
    let mut minRegular: f64 = f64::INFINITY;
    let mut minPrepared: f64 = f64::INFINITY;
    let mut minRead16: f64 = f64::INFINITY;
    let mut minRead0: f64 = f64::INFINITY;
    let mut checksum: f64 = 0.0;

    for round in 0..Rounds
    {
      // --- 1. regular path: dlsym + cif_prep on EVERY call ---
      let t: Instant = Instant::now();
      let mut acc: f64 = 0.0;
      for i in 0..Iterations
      {
        let x: f64 = (i + round) as f64 + 1.0;
        acc += libm.call("sqrt").arg::<f64>(x).result::<f64>()?;
      }
      minRegular = minRegular.min(t.elapsed().as_secs_f64() * 1e6 / Iterations as f64);
      checksum += acc;

      // --- 2. prepared path: dlsym + cif_prep done ONCE above ---
      let t: Instant = Instant::now();
      let mut accPrepared: f64 = 0.0;
      for i in 0..Iterations
      {
        let x: f64 = (i + round) as f64 + 1.0;
        accPrepared += sqrt.call().arg(x).result::<f64>()?;
      }
      minPrepared = minPrepared.min(t.elapsed().as_secs_f64() * 1e6 / Iterations as f64);
      checksum += accPrepared;

      // --- 3. read 16 bytes: request/response with a small payload ---
      let t: Instant = Instant::now();
      for _ in 0..Iterations { let _ = mem.read()?; }
      minRead16 = minRead16.min(t.elapsed().as_secs_f64() * 1e6 / Iterations as f64);

      // --- 4. read 0 bytes: the closest thing to an empty round trip ---
      let t: Instant = Instant::now();
      for _ in 0..Iterations
      {
        let _ = Scope::readMemory(mem.asPointer(), 0)?;
      }
      minRead0 = minRead0.min(t.elapsed().as_secs_f64() * 1e6 / Iterations as f64);
    }

    Ok((minRegular, minPrepared, minRead16, minRead0, checksum))
  }).expect("benchmark scope failed");

  println!("per-call average over one ffi! scope (min of {Rounds} rounds):");
  println!("  1. Call (dlsym+cif every call):  {regularUs:8.3} us");
  println!("  2. CallPrepared (both once):     {preparedUs:8.3} us");
  println!("  3. ReadMemory 16B (with payload): {read16Us:7.3} us");
  println!("  4. ReadMemory 0B (empty reply):  {read0Us:8.3} us");
  println!();
  println!("saved by preparing:  {:.3} us/call ({:.1}% of the regular path)",
    regularUs - preparedUs, 100.0 * (regularUs - preparedUs) / regularUs);
  println!("dlsym+cif share of the regular path: {:.1}%",
    100.0 * (regularUs - preparedUs) / (regularUs - read0Us));
}

// =================================================================================================
