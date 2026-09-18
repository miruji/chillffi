//! Windows implementation of the [`crate::sys`] layer.
//!
//! `kernel32` is already linked into every Rust binary on Windows, so the
//! handful of Win32 calls below are declared directly rather than pulling
//! in `windows-sys`.
// =================================================================================================
use crate::sys::ProcessId;
use std::ffi::c_void;
// =================================================================================================

type Handle = *mut c_void;

const ProcessTerminate: u32 = 0x0001;
const Synchronize: u32 = 0x0010_0000;
const Infinite: u32 = 0xFFFF_FFFF;
const SilentErrorMode: u32 = 0x0001 | 0x0002 | 0x8000; // SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX
const ModuleHandleFromAddress: u32 = 0x0002 | 0x0004; // GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT | ..._FROM_ADDRESS

#[link(name = "kernel32")]
unsafe extern "system"
{
  fn OpenProcess(desiredAccess: u32, inheritHandle: i32, processId: u32) -> Handle;
  fn TerminateProcess(process: Handle, exitCode: u32) -> i32;
  fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
  fn CloseHandle(object: Handle) -> i32;
  fn GetLastError() -> u32;
  fn SetErrorMode(mode: u32) -> u32;
  fn GetModuleHandleExW(flags: u32, moduleName: *const u16, module: *mut Handle) -> i32;
}

unsafe extern "C"
{
  fn _errno() -> *mut i32;
}

// =================================================================================================

/// No-op: Windows has neither SIGCHLD nor zombie processes.
pub const fn ignoreChildExits() -> () {}

/// A clone is a crash domain, not a cooperating peer — kill it outright.
pub fn killProcess(pid: ProcessId) -> ()
{
  let process: Handle = unsafe{ OpenProcess(ProcessTerminate, 0, pid) };
  if process.is_null() { return; }

  unsafe{ TerminateProcess(process, 1) };
  unsafe{ CloseHandle(process) };
}

/// Blocks until the process terminates.
pub fn waitProcess(pid: ProcessId) -> ()
{
  let process: Handle = unsafe{ OpenProcess(Synchronize, 0, pid) };
  if process.is_null() { return; }

  unsafe{ WaitForSingleObject(process, Infinite) };
  unsafe{ CloseHandle(process) };
}

/// Disables Windows Error Reporting for the calling process. Called once at
/// clone startup — a crash must kill the clone immediately, not open a WER
/// dialog the Runtime would sit blocked waiting on.
pub fn silenceCrashReporting() -> ()
{
  unsafe{ SetErrorMode(SilentErrorMode) };
}

// =================================================================================================

/// Base load address of the module containing this function.
pub fn moduleBase() -> usize
{
  let mut module: Handle = std::ptr::null_mut();
  let found: i32 = unsafe{
    GetModuleHandleExW(
      ModuleHandleFromAddress,
      moduleBase as *const () as *const u16,
      &mut module
    )
  };
  if found == 0 { return 0; }

  module as usize
}

// =================================================================================================

/// Reads `errno` of the calling thread. Per-CRT-instance, not per-process:
/// a library statically linked against its own CRT keeps its own copy.
pub fn readErrno() -> i32
{
  unsafe{ *_errno() }
}

/// `GetLastError` of the calling thread, captured alongside [`readErrno`].
/// Most Win32 functions report failure here, not through `errno`.
pub fn readOsError() -> Option<u32>
{
  Some(unsafe{ GetLastError() })
}

// =================================================================================================

/// Alignment plain `malloc` already guarantees on this CRT (`2 *
/// sizeof(void*)`: 16 bytes on x64, 8 on x86 — matches `max_align_t`).
const MallocAlignment: usize = crate::sys::MinAlignment * 2;

thread_local! {
  /// Pointers handed out via `_aligned_malloc`, which need `_aligned_free`
  /// rather than plain `free`. A clone serves one request at a time (see
  /// `cloneLoop`), so thread-local bookkeeping is enough — no locking needed.
  static AlignedAllocations: std::cell::RefCell<std::collections::HashSet<usize>> =
    std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Plain `malloc`. Must stay plain: `Scope::free` also has to release
/// pointers from arbitrary C-side allocators (e.g. a `malloc` called
/// through FFI directly), and those are never in [`AlignedAllocations`].
pub fn allocate(length: usize) -> *mut c_void
{
  unsafe{ libc::malloc(length) }
}

/// Plain `malloc` already satisfies alignment up to [`MallocAlignment`];
/// only larger requests need `_aligned_malloc`, tracked in
/// [`AlignedAllocations`] so [`deallocate`] knows to release it with
/// `_aligned_free` instead of `free`.
pub fn allocateAligned(length: usize, alignment: usize) -> Result<*mut c_void, String>
{
  if alignment <= MallocAlignment
  {
    let pointer: *mut c_void = unsafe{ libc::malloc(length) };
    if pointer.is_null()
    {
      return Err(format!("malloc failed for {} bytes", length));
    }
    return Ok(pointer);
  }

  let pointer: *mut c_void = unsafe{ libc::aligned_malloc(length, alignment) };
  if pointer.is_null()
  {
    return Err(format!(
      "_aligned_malloc failed for {} bytes at alignment {}", length, alignment
    ));
  }
  AlignedAllocations.with(|set| { set.borrow_mut().insert(pointer as usize); });
  Ok(pointer)
}

/// `free`, unless `pointer` is a tracked `_aligned_malloc` result, in which
/// case `_aligned_free` — the two are not interchangeable on Windows.
pub fn deallocate(pointer: *mut c_void) -> ()
{
  let wasAligned: bool =
    AlignedAllocations.with(|set| set.borrow_mut().remove(&(pointer as usize)));

  if wasAligned
  {
    unsafe{ libc::aligned_free(pointer) };
  }
  else
  {
    unsafe{ libc::free(pointer) };
  }
}

// =================================================================================================
