use chillffi::callback;
use chillffi::callvPointer;
use chillffi::ffi::value::{Value, Type, Pointer};
use chillffi::ffi;
// =================================================================================================

/// Verify signal()'s returned "previous handler" pointer is real and callable.
fn main() -> ()
{
  // signal() both takes and returns a function pointer — the case
  // callPointer! exists for: calling an address we didn't get via dlsym.
  ffi!(|scope| {
    let libc: Library = scope.load("libc.so.6")?;

    // Register a Rust closure as SIGUSR1's handler.
    let handler = callback!([] |args: Vec<Value>| -> Value {
      let Value::I32(signum) = args[0] else { panic!("expected i32 signum") };
      println!("[handler] called directly via callPointer!, signum = {signum}");
      Value::None
    });
    let handler: Value = scope.callback(vec![Type::I32], Type::None, handler);

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
    callvPointer!(scope, old, 10 as i32)?;

    Ok(())
  }).expect("signal roundtrip failed");

  //
  println!("OK: the pointer signal() returned was a real, callable callback");
}

// =================================================================================================