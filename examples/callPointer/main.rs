#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::{LibcPath, SignalNumber};
// =================================================================================================
use chillffi::callback;
use chillffi::callvPointer;
use chillffi::ffi;
use chillffi::ffi::types::primitive::{Callback, Pointer};
// =================================================================================================

/// `callPointer` / `callvPointer!` — call a known address, no dlopen involved.
fn main() -> ()
{
  callvPointerOnSignalReturnedHandler();
}

// =================================================================================================

/// Get the previous handler address from `signal()`, then call it directly.
fn callvPointerOnSignalReturnedHandler() -> ()
{
  ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;

    let handler: Callback = callback!(scope, |signum: i32| -> () {
      println!("[handler] called directly via callvPointer!, signum = {signum}");
    });

    // Install it. The signal is never raised — signal() only stores and
    // returns pointers, delivery is irrelevant here.
    libc.call("signal").arg::<i32>(SignalNumber).arg(handler).void()?;

    // Restore SIG_DFL and capture what signal() reports as "previous" —
    // has to be the exact address just installed above.
    let old: Pointer = libc.call("signal").arg::<i32>(SignalNumber).arg(Pointer(0)).result()?;

    // Call that address directly, bypassing signal() entirely.
    callvPointer!(scope, old, SignalNumber)?;

    Ok(())
  }).expect("signal roundtrip failed");

  println!("ok: the pointer signal() returned was a real, callable callback");
}

// =================================================================================================
