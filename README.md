## chillffi

**A simple isolated dynamic FFI framework for Rust**

[![Crates.io](https://img.shields.io/crates/v/chillffi.svg)](https://crates.io/crates/chillffi)
[![Documentation](https://docs.rs/chillffi/badge.svg)](https://docs.rs/chillffi)
[![License: FCL](https://img.shields.io/badge/License-FCL-blue.svg)](LICENSE.md)

`chillffi` allows dynamically loading **C ABI-compatible libraries** 
and calling their functions at runtime, **isolating each FFI call in a separate process**.
If third-party native code crashes or corrupts memory, 
the failure is contained within the isolated process, 
keeping your main Rust application running.

---

## ✨ Features

| Feature                  | Status                                                                              |
|--------------------------|-------------------------------------------------------------------------------------|
| Crash isolation          | ✅ A separate process for FFI that does not break your Runtime.                      |
| Native FFI execution     | ✅ Not a VM, not large in size, and does not require large dependencies.             |
| Startup speed            | ✅ The Zygote model does not retain garbage and uses a fast `fork` for each FFI.     |
| IPC                      | ✅ `ipc-channel` provides serialization and is implemented for different platforms.  |
| Multithreading and async | ✅ Does not break with multithreading and async                                      |
| Scope                    | ✅ Scope for FFI execution areas (`ffi!`); so that they are short and do not escape. |
| Retained Scope           | ✅ Temporary retention of the FFI scope for dynamic systems.                         |
| Dynamic loading          | ✅ `libffi` is simple, stable, cross-platform, and small in size.                    |
| Path resolver            | ✅ Global, scope-level, and direct path resolver for libraries.                      |
| Static FFI               | ✅ Through Rust code.                                                                |
| Dynamic FFI              | ✅ On-the-fly function calls without the need to compile static C bindings.          |
| Static structures        | ✅ `repr` structures.                                                                |
| Dynamic structures       | ✅ Reading and writing structures with arbitrary layouts.                            |
| Pointer-based structures | ✅ Support for passing structures through pointers.                                  |
| By-value structures      | ✅ Support for passing structures by value.                                          |
| Allocation handling      | ✅ Allocation of a memory region for FFI.                                            |
| Callbacks                | ✅ Passing closures as C functions (`callback!`).                                    |
| Signals                  | ✅ Working with signals and calling pointers (`callvPointer`, `callPointer`).        |
| Errno Policy             | ✅ Configuring errno reading at the call, scope, or global level.                    |
| String data types        | ✅ String (`""`), CString (`c""`), RawString (`b""`).                                |
| Variadic arguments       | ✅ Support for variable arguments in function calls.                                 |
| Concurrency limit        | ✅ Caps simultaneously live clones; blocks (not errors) when full. See [`limits`](src/limits.rs) / [`stats`](src/stats.rs). |
| Live stats               | ✅ `activeClones()`, `availableSlots()`, `zygotesAlive()`, `hotAlive()`.            |
| Sandbox (FS protection)  | ⏳ [#45](https://github.com/rts-lang/chillffi/issues/45)                             |
| Libraries from bytes     | ⏳ [#42](https://github.com/rts-lang/chillffi/issues/42)                             |

---

## 📦 Installation

Add the dependency to `Cargo.toml`. There are no separate configuration flags.

| Platforms         | Status                                                  |
|-------------------|---------------------------------------------------------|
| Linux             | ✅ Ubuntu 24.04 / 26.04 (x86_64 & ARM64)                 |
| macOS             | ✅ macOS 15 / 26 (Intel & Apple Silicon)                 |
| Windows (MSVC)    | ✅ Windows Server 2025 (x86_64) and Windows 11 ARM       |
| Windows (GNU)     | ❌ `libffi-sys` has no vendored build for this toolchain |
| WASM              | ⏳ [#43](https://github.com/rts-lang/chillffi/issues/43) |
| Bare metal        | ⏳ [#44](https://github.com/rts-lang/chillffi/issues/44) |
| Build as `cdylib` | ❌                                                       |

> **Supported architectures only:** `x86_64` and `aarch64` on the platforms above.
> Other architectures (e.g. `x86`, `arm`, `riscv64`, …) are not supported.
>
> 32-bit is not supported because the framework targets only 64-bit and
> modern architectures — they can be checked and are used more widely.
>
> All because verification is done through GitHub CI, and there are no other platforms there.
> If you need other platforms — that is separate work and research.

## 🚀 Quick Start

Example of a safe call to the `sqrt` function from the system library `libm.so.6` 
using the `ffi!{}` macro and explicit typing:
```rust
fn main() -> ()
{
  // Perform an FFI call inside an isolated context using a macro
  let result: f64 = ffi!(|scope| {
    // Dynamically load the system library
    let libm: Library = scope.load("libm.so.6")?;
  
    // Call the "sqrt" function, specifying the expected return type
    libm.call("sqrt").arg::<f64>(4.0).result()
    
    // Here libm will be automatically cleared due to drop() when exiting the closure.
    // You can also do this manually via drop(libm) or libm.unload()?
  }).expect("FFI call failed");

  // Process the typed result
  println!("sqrt(4.0) = {}", result);
  assert!((result - 2.0).abs() < f64::EPSILON, "sqrt(4.0) != 2.0");
}
```

Example of a memory-sensitive call to the `clock_gettime` function 
from the system library `libc.so.6` using `AllocatedMemory`:
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

The tests there are divided by features, and inside there are different usage variations.

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

## 🧮 Process pools & concurrency limit

Each `ffi!{}` block forks a clone of the Main Zygote. With no coordination
between threads, `N` concurrent `ffi!{}` blocks produce `N` live clones —
and a workload that opens more of them in parallel than the OS allows
becomes a fork-bomb: the next `fork()` returns `EAGAIN` (Unix) or
`ERROR_MAX_THRDS` (Windows), and the call surfaces as `SpawnFailed`.
Worse, those `N` clones count against the **per-user** `RLIMIT_NPROC`
(Linux/macOS), which is shared with the shell, system daemons, and any
**other** chillffi process running under the same user.

chillffi installs a single global counting semaphore that caps the number
of simultaneously live clones. Reaching the cap **blocks** the next
`ffi!{}` block — it does not error.

### The three pools

```text
zygotePool       ─ Main Zygotes ready to fork a clone on demand.
                   Default: 1 (today: exactly one Main Zygote per
                   chillffi process; `> 1` is reserved for future work).

hotPool          ─ Long-lived clone processes kept warm between `ffi!{}`
                   blocks for low-latency reuse.
                   Default: 0 (today: every `ffi!{}` block gets a fresh
                   clone from the Main Zygote and kills it on scope
                   exit). Reusing a clone across blocks would carry
                   `dlopen`'d state from one block into the next, which
                   breaks the "sterile process per FFI block" guarantee;
                   `> 0` is reserved for the explicit retained-scope API.

concurrencyLimit ─ Hard ceiling on simultaneously live clones, summed
                   across all threads and all `ffi!{}` blocks in this
                   chillffi process. Reach it and the next `ffi!{}`
                   block **waits** for a slot — never errors just
                   because the limit is full.
```

### Defaults

The default `concurrencyLimit` is computed at first use from the
OS-imposed per-user and system-wide thread limits, minus a buffer:

```text
limit = min(userSoft, systemHard, RECOMMENDED_HARD_CAP) - buffer
```

Conservative constants:

| Constant                | Value | Why |
|-------------------------|-------|-----|
| `RECOMMENDED_HARD_CAP`  | 32    | Above 32 concurrent clones the per-call `fork` + IPC bootstrap starts to dominate wall time on a typical machine, and a single chillffi process should not be able to drown the shell. |
| `defaultBuffer`         | 16    | Headroom left for the shell, system daemons, and any **other** chillffi instance running under the same user. chillffi does not own the per-user limit. |

OS autodetect sources:

| OS      | Per-user soft              | Per-user hard              | System-wide                          |
|---------|----------------------------|----------------------------|--------------------------------------|
| Linux   | `getrlimit(RLIMIT_NPROC)`  | `getrlimit(RLIMIT_NPROC)`  | `/proc/sys/kernel/threads-max`       |
| macOS   | `getrlimit(RLIMIT_NPROC)`  | `getrlimit(RLIMIT_NPROC)`  | `sysctl(kern.maxproc)`                |
| Windows | not exposed                | not exposed                | conservative constant (2048)         |

| OS      | Typical soft | Hard limit     | Error at the limit     |
|---------|--------------|----------------|------------------------|
| Linux   | 1024–14248   | 14248–65000    | `EAGAIN`               |
| macOS   | 256–1064     | 2048           | `EAGAIN`               |
| Windows | ~1000–2000   | depends on SKU | `ERROR_MAX_THRDS`      |

### Recommended configurations

- **Default (do nothing):** `16/32` semantics — a buffer of 16 and a cap
  of 32. Conservative, good for most workloads.
- **Above 32:** set `concurrencyLimit` higher only if you understand the
  per-call cost and your system has the headroom. This is "at your own
  risk" territory — the per-user `RLIMIT_NPROC` is the real wall and
  chillffi will not stop you from running into it.
- **Below 16:** perfectly safe; `concurrencyLimit = 1` serialises every
  `ffi!{}` block, which is sometimes what you want for a workload that is
  not parallelisable anyway.

### Inspecting & overriding

```rust
use chillffi::limits;
use chillffi::stats;

// Inspect (works after the first ffi!{} block; reflects auto-detected
// defaults if configure() was never called):
let l = *limits::current();
println!("zygotePool={}, hotPool={}, concurrencyLimit={}",
  l.zygotePool, l.hotPool, l.concurrencyLimit);

// Live counters (lock-free, safe on hot paths):
println!("activeClones={} ({} free), zygotesAlive={}, hotAlive={}",
  stats::activeClones(),
  stats::availableSlots(),
  stats::zygotesAlive(),
  stats::hotAlive());

// Override (call once, before the first ffi!{} block):
limits::configure(limits::Limits {
  zygotePool: 1,
  hotPool: 0,
  concurrencyLimit: 8
}).expect("invalid limits");
```

### Known limitation (0.4)

`zygotePool > 1` and `hotPool > 0` are accepted by `limits::configure`
for forward compatibility but **not yet acted on** — chillffi still runs
with a single Main Zygote and creates a fresh clone per `ffi!{}` block.
The fields are recorded and validated so a future 0.x can wire them up
without a breaking change to the public API. Tracked as `todo` at the
top of `src/zygote.rs` (sections 1, 2, and 4 of the existing plan).

## 📄 License

The source code is distributed under the [FCL](LICENSE.md) license.
This is a custom license of the [RTS](https://github.com/rts-lang/rts) programming language.

In addition, **chillffi** is distributed under this license because the source code was
originally taken from the **RTS** language itself. Therefore, **chillffi** inherits this license.

For a more accurate understanding, you should familiarize yourself with the text of the license.

But if very simply, for those who just work and want to use it:
- Personal/non-commercial use ➞ free
- Commercial use without modifications ➞ free
- Commercial use with modifications ➞ requires the author's permission or opening the changes

Keep this in mind for your projects.

<!-- ## 🧠 Contributing -->