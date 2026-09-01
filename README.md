## chillffi

**A simple isolated dynamic FFI framework for Rust**

[![Crates.io](https://img.shields.io/crates/v/chillffi.svg)](https://crates.io/crates/chillffi)
[![Documentation](https://docs.rs/chillffi/badge.svg)](https://docs.rs/chillffi)
[![License: FCL](https://img.shields.io/badge/License-FCL-blue.svg)](LICENSE.md)

`chillffi` allows dynamically loading C libraries `.so` 
and calling their functions at runtime, **isolating the calls in a separate empty process**.

If third-party C code crashes or corrupts something, your main Rust application will continue running.

_(In the future, an expansion of the functionality for working with FFI is planned.)_

---

## ✨ Features

- 🛡️ **Crash Isolation**: A crash or panic inside unreliable FFI code does not break or corrupt the main process.
- ⚡ **Zygote Model (Zygote)**: Fast forking and spawning of isolated workers with minimal overhead.
- 🚀 **In-memory IPC**: Transfer of file descriptors and data through sockets without accessing the disk.
- 🧩 **Dynamic FFI**: On-the-fly function calls without the need to compile static C bindings.

---

## 📦 Installation

Add the dependency to `Cargo.toml`:

> [!NOTE]
>
> Supported only on Unix-like OSes, and tested only on Linux.
>
> _(Planned: Windows, macOS, WASM, Bare metal.)_

## 🚀 Quick Start

Example of a safe call to the `sqrt` function from the system library `libm.so.6` using the `ffi!{}` macro and explicit typing:

```rust
fn main() -> ()
{
  // Perform an FFI call inside an isolated context using a macro
  let result: f64 = ffi!(|scope| {
    // Dynamically load the system library
    let libm: Library = scope.load("libm.so.6")?;
  
    // Call the "sqrt" function, specifying the expected return type
    Ok( libm.call("sqrt").arg::<f64>(4.0).result()? )
    
    // Here libm will be automatically cleared due to drop() when exiting the closure.
    // You can also do this manually via drop(libm) or libm.unload()?
  }).expect("FFI call failed");

  // Process the typed result
  println!("sqrt(4.0) = {}", result);
  assert!((result - 2.0).abs() < f64::EPSILON, "sqrt(4.0) != 2.0");
}
```

For memory-sensitive operations — C strings, out-parameters, or raw buffers — 
use the scoped variant with `Scope` and `AllocatedMemory`:
```rust
fn main() -> ()
{
  // clock_gettime(CLOCK_REALTIME, &timespec) — struct out-param via Alloc/ReadMemory,
  // the case a plain Value::Pointer can't cover on its own.
  let (secs, nanos): (i64, i64) = ffi!(|scope| {
    let libc: Library = scope.load("libc.so.6")?;

    // struct timespec { time_t tv_sec; long tv_nsec; } — 16 bytes on x86_64 Linux
    let mem: AllocatedMemory = scope.alloc(16)?;

    libc.call("clock_gettime")
      .arg::<i32>(0 /* CLOCK_REALTIME */)
      .arg(mem.asPointer())
      .void()?;

    let Value::RawString(bytes) = mem.read()? else { 
      panic!("expected bytes")
    };
    drop(mem);

    let secs: i64 = i64::from_ne_bytes(bytes[0..8].try_into().unwrap());
    let nanos: i64 = i64::from_ne_bytes(bytes[8..16].try_into().unwrap());
    Ok((secs, nanos))
  }).expect("clock_gettime failed");

  println!("clock_gettime(CLOCK_REALTIME) = {}.{:09}", secs, nanos);
}
```

For more detailed examples, see the [examples](examples) folder.

You can also run them via `cargo run --example <name>`.

## ⚡ Why is this convenient

In general practice, we are used to doing it like in Python 
and other programming languages - precisely specifying all the wrappers for FFI.
After which we observe how FFI still crashes anyway 
and the libraries are not built, and the code does not work.

This is all because FFI requires a manual bridge and it is not always possible to make one.

**chillffi** works on a different principle - you can write any FFI code inside isolated blocks.
Because FFI should not be scattered throughout your code - this is an unsafe approach.
Therefore, we write it in isolation and preferably briefly, only when necessary.

Since everything is located in isolated processes - 
we do not damage the main runtime in any way and do not touch your code.
All FFI requests work in a sterile manner and 
in case of errors will clearly let you know about it. 
You can also simply ignore them if you want.

As a result, we can freely and simply write:
- Test code
- Educational code
- FFI bridges
- Dynamic programming languages
- Game engines
- Reactive systems and dynamic systems
- And many other things

This is also different from the WASM approach - because we preserve a true native execution here.

## 🛠️ How it works

1. Before your code starts running, a Zygote is created - it is an empty process for cloning itself and isolating FFI.
2. When work with FFI is required - a copy is created from the zygote.
3. Data and descriptors are transferred through a secure socket channel in memory.
4. In case of errors, the supervisor intercepts the worker crash and returns the error to Rust, keeping your application stable.

> [!IMPORTANT]
> 
> This does not protect you from the FFI code running inside the isolated process.
> 
> For example, if it does something with your OS or file system -
> it is already your responsibility to separately protect against this.
> 
> For example: You can use a virtual space for the file system and so on.

> [!IMPORTANT]
> 
> FFI blocks should be as small as possible in size. I.e., not 100 lines in 1 FFI space.
> 
> An exception can be considered when you need a single address space for several operations.
> 
> In other cases, you should separate FFI requests as much as possible.
> 
> Because no one can guarantee that any FFI request will not break your code.
> 
> Even if you are an experienced programmer, there are things that do not depend on your experience.

<!-- ## 🧭 Roadmap (todo better about capabilities) -->

## 📄 License

The source code is distributed under the [FCL](LICENSE.md) license.
This is a custom license of the [RTS](https://github.com/rts-lang/rts) programming language.

In addition, **chillffi** is distributed under this license because the source code was
originally taken from the **RTS** language itself. Therefore, **chillffi** inherits this license.

For a more accurate understanding, you should familiarize yourself with the text of the license.

But if very simply, for those who just work and want to use it:
- Personal/non-commercial use → free
- Commercial use without modifications → free
- Commercial use with modifications → requires the author's permission or opening the changes

Keep this in mind for your projects.

<!-- ## 🧠 Contributing -->