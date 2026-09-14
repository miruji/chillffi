mod platform;
use crate::platform::LibcPath;
// =================================================================================================
use chillffi::callback;
use chillffi::callPointer;
use chillffi::callvPointer;
use chillffi::ffi;
use chillffi::ffi::types::primitive::{Callback, Pointer};
// =================================================================================================

/// Verify signal()'s returned "previous handler" pointer is real and callable
/// through callvPointer! (fire-and-forget, no result type).
fn testCallvPointer() -> ()
{
  // signal() both takes and returns a function pointer — the case
  // callvPointer! exists for: calling an address we didn't get via dlsym.
  ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;

    // Register a Rust closure as SIGUSR1's handler.
    let handler: Callback = callback!(scope, |signum: i32| -> () {
      println!("[handler] called directly via callvPointer!, signum = {signum}");
    });

    // Install it. The signal is never raised — signal() only stores and
    // returns pointers, delivery is irrelevant here.
    libc.call("signal")
      .arg::<i32>(10 /* SIGUSR1 */)
      .arg(handler)
      .void()?;

    // Restore SIG_DFL and capture what signal() reports as "previous" —
    // that has to be the exact address we just installed above.
    let old: Pointer = 
      libc.call("signal")
      .arg::<i32>(10)
      .arg(Pointer(0))
      .result()?;

    // Call that address directly, bypassing signal() entirely.
    callvPointer!(scope, old, 10_i32)?;

    Ok(())
  }).expect("signal roundtrip failed");

  //
  println!("OK: the pointer signal() returned was a real, callable callback (callvPointer!)");
}

/// Same mechanics as above, but the handler now returns a value —
/// so we reach for callPointer!, which *does* carry a result type.
fn testCallPointer() -> ()
{
  // Unlike callvPointer!, callPointer! has a result type — that is the
  // whole reason the two macros exist side by side.
  ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;

    // Register a Rust closure that returns i32 instead of ().
    let doubler: Callback = callback!(scope, |x: i32| -> i32 { x * 2 });

    // Install it via signal() so we can recover its raw address below —
    // same trick as in testCallvPointer(), just with a value-returning closure.
    libc.call("signal")
      .arg::<i32>(10 /* SIGUSR1 */)
      .arg(doubler)
      .void()?;

    // signal()'s "previous handler" return is our raw address.
    let raw: Pointer =
      libc.call("signal")
      .arg::<i32>(10)
      .arg(Pointer(0))
      .result()?;

    // callPointer! *does* have a result type — here it is inferred as i32.
    let result: i32 = callPointer!(scope, raw, 21_i32)?;
    println!("[handler] returned {result} via callPointer!");

    Ok(())
  }).expect("callPointer! roundtrip failed");

  //
  println!("OK: callPointer! carried a typed result back from a raw callback");
}

// =================================================================================================

fn main() -> ()
{
  testCallvPointer();
  testCallPointer();
}

// =================================================================================================