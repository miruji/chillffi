#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::{
  CloseSymbolName, LibcPath, PipeSymbolName, ReadSymbolName, StrdupSymbolName, WriteSymbolName,
};
use crate::platform::platformExt;
// =================================================================================================
use chillffi::ffi;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::errors::FFIError;
use chillffi::ffi::scope::Scope;
use chillffi::ffi::types::primitive::Pointer;
// =================================================================================================

/// Four ownership boundaries when moving data across the FFI isolation boundary.
///
/// 1. **Rust data** — owned on the Runtime side, copied into the clone for the
///    duration of a single call. C must not keep the pointer after the call returns.
/// 2. **chillffi allocation** (`scope.alloc` → `AllocatedMemory`) — allocated on
///    the clone's heap, owned by us, freed automatically on `Drop`.
/// 3. **C allocation, we free** (`malloc` / `strdup` + `Scope::free`) — C allocated,
///    we release through the same underlying allocator. `libc.call("free")` is an
///    equivalent fallback when the pointer came from the same CRT.
/// 4. **C owns the full lifecycle** — C allocates *and* frees inside its own code.
///    No pointer escapes; Rust never calls `Scope::free` or `free`. See `owned.c`.
///
/// C's allocator is not wrapped in RAII on purpose: only C knows the true
/// allocation shape. chillffi refuses to pretend it owns what it did not create.
///
/// When C *does* return an opaque multi-buffer object that only its destroy API
/// can release, that is still "C owns cleanup" — see `examples/dynamicStruct`
/// (`process` / `freeData`). The difference from case 4 here is that a destroy
/// call is still required from Rust; the free itself stays on the C side.
fn main() -> ()
{
  rustOwnedInput();
  chillffiAllocation();
  cAllocationWeFree();
  cOwnsFullLifecycle();
}

// =================================================================================================

/// 1. Rust-owned input.
///
/// `Vec<u8>`, `&str`, `CString` are serialized over IPC into the clone for the
/// duration of the call. After the call returns the temporary buffer in the
/// clone is gone — storing that pointer inside C would be a use-after-free.
///
/// Safe pattern: pass data C only *reads* during the call (strlen, atoi, …).
/// Unsafe pattern: pass a pointer that C keeps for later (globals, callback tables).
fn rustOwnedInput() -> ()
{
  let len: usize = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;

    // Rust owns the bytes. IPC places a temporary copy in the clone;
    // strlen only reads it for this call and does not retain the pointer.
    let len: usize = libc.call("strlen").arg(c"hello from rust").result()?;
    Ok(len)
  }).expect("rust-owned input failed");

  assert_eq!(len, 15);
  println!("ok: rust-owned input   — IPC copy lives only for the call, len = {len}");
}

// =================================================================================================

/// 2. chillffi allocation (`AllocatedMemory`).
///
/// `scope.alloc` asks the clone to `malloc` on *its* heap and wraps the address
/// in `AllocatedMemory<'g>`. The lifetime `'g` is the `ffi!` block: the value
/// cannot escape, and `Drop` sends `Free` automatically.
///
/// Use this when *we* need a buffer that C will write into (out-params, structs
/// filled by the callee) or that we will pass to several calls inside the same
/// scope. We own the region; C only borrows the pointer for the duration of
/// each call.
fn chillffiAllocation() -> ()
{
  let received: Vec<u8> = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;

    // int pipefd[2]; — out-parameter that pipe() fills.
    let fdsMem: AllocatedMemory = scope.alloc(8)?;

    #[cfg(unix)]
    let result: i32 = libc.call(PipeSymbolName).arg(fdsMem.asPointer()).result()?;
    #[cfg(windows)]
    let result: i32 = libc.call(PipeSymbolName)
      .arg(fdsMem.asPointer())
      .arg::<u32>(4096)
      .arg::<i32>(0x8000)
      .result()?;
    if result != 0 {
      return Err(FFIError::Other("pipe() failed".into()));
    }

    let fdsBytes: Vec<u8> = fdsMem.read()?;
    let readFd: i32 = i32::from_ne_bytes(fdsBytes[0..4].try_into().unwrap());
    let writeFd: i32 = i32::from_ne_bytes(fdsBytes[4..8].try_into().unwrap());

    // Second buffer we own — C writes into it via read().
    let bufMem: AllocatedMemory = scope.alloc(2)?;

    #[cfg(unix)]
    {
      libc.call(WriteSymbolName).arg(writeFd).arg(b"hi".to_vec()).arg::<usize>(2).void()?;
      libc.call(ReadSymbolName).arg(readFd).arg(bufMem.asPointer()).arg::<usize>(2).void()?;
    }
    #[cfg(windows)]
    {
      libc.call(WriteSymbolName).arg(writeFd).arg(b"hi".to_vec()).arg::<u32>(2).void()?;
      libc.call(ReadSymbolName).arg(readFd).arg(bufMem.asPointer()).arg::<u32>(2).void()?;
    }

    let readBytes: Vec<u8> = bufMem.read()?;

    libc.call(CloseSymbolName).arg(readFd).void()?;
    libc.call(CloseSymbolName).arg(writeFd).void()?;

    // fdsMem and bufMem Drop here → Free is sent to the clone. No manual free.
    // Although manual free could be done if necessary.
    Ok(readBytes)
  }).expect("chillffi allocation failed");

  assert_eq!(received, b"hi".to_vec());
  println!("ok: chillffi allocation — AllocatedMemory RAII free, pipe -> {:?}",
    String::from_utf8_lossy(&received));
}

// =================================================================================================

/// 3. C allocation, we free (`strdup` + `Scope::free`).
///
/// C creates the buffer and returns a raw `Pointer`. We did not allocate it,
/// so there is no `AllocatedMemory` and no automatic `Drop`. Ownership stays
/// with C's allocator until we explicitly release it.
///
/// Read/write go through `Scope::readMemory` / `Scope::writeMemory`.
/// Release goes through `Scope::free` — valid when the pointer came from the
/// same underlying allocator chillffi uses (`malloc` / CRT on the supported
/// platforms). Equivalent fallback: `libc.call("free").arg(ptr).void()?`.
///
/// If the library has its own heap or a paired destroy API, do not use this —
/// call that API instead (see `examples/dynamicStruct` + `freeData`).
fn cAllocationWeFree() -> ()
{
  let copy: Vec<u8> = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;

    // char *strdup(const char *s) — C allocates, we only receive the address.
    let ptr: Pointer = libc.call(StrdupSymbolName).arg(c"owned by C").result()?;
    if usize::from(ptr) == 0 {
      return Err(FFIError::Other("strdup returned null".into()));
    }

    // Borrow the bytes without taking ownership of the allocation.
    let bytes: Vec<u8> = Scope::readMemory(ptr, 10)?; // "owned by C" is 10 bytes + NUL
    assert_eq!(&bytes[..10], b"owned by C");

    // Optional mutation through the same channel.
    Scope::writeMemory(ptr, c"OWNED BY C")?;

    let after: Vec<u8> = Scope::readMemory(ptr, 10)?;
    assert_eq!(&after[..10], b"OWNED BY C");

    // Manual release — we are not the allocator, so Drop will not help.
    // Fallback when you prefer the C symbol explicitly:
    //   libc.call("free").arg(ptr).void()?;
    Scope::free(ptr)?;

    Ok(after)
  }).expect("C allocation (we free) failed");

  assert_eq!(&copy[..10], b"OWNED BY C");
  println!("ok: C alloc, we free   — strdup + Scope::readMemory/free, bytes = {:?}",
    String::from_utf8_lossy(&copy[..10]));
}

// =================================================================================================

/// 4. C owns the full lifecycle (`owned.c`).
///
/// C allocates, uses, and frees *inside* the call. No pointer is returned to
/// Rust, so there is nothing to `Scope::free` or `free`. This is the cleanest
/// form of "C manages its own memory": ownership never crosses the FFI boundary.
///
/// ```c
/// int processOwned(void);       // malloc → work → free → return sum
/// int measureOwnedCopy(char*);  // malloc copy → strlen → free → return len
/// ```
///
/// Contrast with `examples/dynamicStruct`: there C *returns* an opaque object
/// and Rust must later call `freeData` — still C's free logic, but a destroy
/// call is part of the public API. Here even that call is unnecessary.
fn cOwnsFullLifecycle() -> ()
{
  let (sum, len): (i32, i32) = ffi!(|scope| {
    scope.addSearchPath("examples/memory");
    let lib: Library = scope.load(platformExt!("libowned"))?;

    // processOwned: buffer lives only inside C for this call.
    let sum: i32 = lib.call("processOwned").result()?;

    // measureOwnedCopy: internal strdup-like copy, freed before return.
    let len: i32 = lib.call("measureOwnedCopy").arg(c"hello").result()?;

    Ok((sum, len))
  }).expect("C owns full lifecycle failed");

  assert_eq!(sum, 60);
  assert_eq!(len, 5);
  println!("ok: C owns lifecycle   — processOwned={sum}, measureOwnedCopy={len} (no free from Rust)");
}

// =================================================================================================
