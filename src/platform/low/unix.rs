//! Unix implementation of the [`low`] layer.
// =================================================================================================
use std::ffi::c_void;
use crate::platform::low;
// =================================================================================================

/// Lets the kernel reap terminated clones so the main zygote never
/// accumulates zombies. Must only be called there — with SIGCHLD ignored
/// in the Runtime, `waitpid` in `supervisorLoop` would fail with `ECHILD`.
///
/// Linux has its own `SIGCHLD, SIG_IGN` call inside `platform::ipc::linux`,
/// so this stays macOS-only.
#[cfg(target_os = "macos")]
pub fn ignoreChildExits() -> ()
{
  unsafe{ libc::signal(libc::SIGCHLD, libc::SIG_IGN); }
}

/// A clone is a crash domain, not a cooperating peer — kill it outright.
pub fn killProcess(pid: low::ProcessId) -> ()
{
  unsafe{ libc::kill(pid as libc::pid_t, libc::SIGKILL); }
}

/// Blocks until the process terminates.
pub fn waitProcess(pid: low::ProcessId) -> ()
{
  unsafe{ libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), 0); }
}

// =================================================================================================

/// Base load address of the module containing this function.
pub fn moduleBase() -> usize
{
  let mut info: libc::Dl_info = unsafe{ std::mem::zeroed() };
  unsafe{ libc::dladdr(moduleBase as *const () as *const c_void, &mut info) };
  info.dli_fbase as usize
}

// =================================================================================================

/// Reads `errno` of the calling thread.
pub fn readErrno() -> i32
{
  #[cfg(target_os = "linux")]
  { unsafe{ *libc::__errno_location() } }

  #[cfg(target_os = "macos")]
  { unsafe{ *libc::__error() } }

  #[cfg(not(any(target_os = "linux", target_os = "macos")))]
  { std::io::Error::last_os_error().raw_os_error().unwrap_or(0) }
}

/// Unix has no second error channel besides `errno`.
pub const fn readOsError() -> Option<u32>
{
  None
}

// =================================================================================================

/// Plain `malloc`.
pub fn allocate(length: usize) -> *mut c_void
{
  unsafe{ libc::malloc(length) }
}

/// `posix_memalign`. `alignment` is already normalized to a power of two
/// no smaller than [`low::MinAlignment`].
pub fn allocateAligned(length: usize, alignment: usize) -> Result<*mut c_void, String>
{
  let mut pointer: *mut c_void = std::ptr::null_mut();
  let code: i32 = unsafe{ libc::posix_memalign(&mut pointer, alignment, length) };
  if code != 0
  {
    return Err(format!("posix_memalign failed with code {}", code));
  }
  Ok(pointer)
}

/// `free` — valid for pointers from both [`allocate`] and [`allocateAligned`].
pub fn deallocate(pointer: *mut c_void) -> ()
{
  unsafe{ libc::free(pointer) };
}

// =================================================================================================
