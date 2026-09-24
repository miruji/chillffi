//! Unix implementation of the [`low`] layer.
// =================================================================================================
use std::ffi::c_void;
use crate::platform::low;
// =================================================================================================

/// Lets the kernel reap terminated clones so the main zygote never
/// accumulates zombies. Must only be called there — with SIGCHLD ignored
/// in the Runtime, `waitpid` in `supervisorLoop` would fail with `ECHILD`.
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


/// Cross-platform shape of an OS-imposed process / thread cap
/// (see [`detectProcessLimits`]).
///
/// All values are **best-effort** — `None` means "could not be read on this
/// platform / with this kernel". A reader must treat `None` as "unknown, do
/// not rely on this cap". Any `Some` value is the raw OS-imposed number; the
/// usable chillffi limit is computed on top (see [`crate::limits`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct OsProcessLimits
{
  /// Per-user soft limit on processes/threads (Linux/macOS `RLIMIT_NPROC`).
  /// `None` where the OS exposes no such concept (Windows).
  pub userSoft: Option<u64>,

  /// Per-user hard limit on processes/threads (Linux/macOS `RLIMIT_NPROC`).
  /// `None` where the OS exposes no such concept (Windows).
  pub userHard: Option<u64>,

  /// System-wide cap on threads (`kernel.threads-max` on Linux,
  /// `kern.maxproc` on macOS). `None` where not readable.
  pub systemHard: Option<u64>
}

/// Best-effort read of the OS-imposed process / thread caps that matter for
/// deciding how many simultaneous chillffi clones can exist.
///
/// chillffi may run alongside other programs under the same user, and the
/// user's `RLIMIT_NPROC` is shared across **all** of them — that is why a
/// chillffi process cannot simply grab the entire soft limit for itself.
/// The buffer ([`crate::limits::defaultBuffer`]) is the headroom we leave
/// for the shell, system daemons, and any other chillffi instances.
///
/// Platform notes:
/// - **Linux**: `getrlimit(RLIMIT_NPROC)` + `/proc/sys/kernel/threads-max`.
///   `RLIMIT_NPROC` is `RLIM_INFINITY` (`u64::MAX`) on most distros by
///   default; in that case the real cap is `kernel.threads-max`.
/// - **macOS**: `getrlimit(RLIMIT_NPROC)` (returns the per-user value
///   `ulimit -u` exposes) + `sysctl(kern.maxproc)`.
/// - **Windows**: no per-user process cap exposed through a stable API.
///   Returns `None` for both user fields and a conservative constant for
///   `systemHard`. The real ceiling is whatever the kernel+commit-charge
///   allows; chillffi still benefits from the buffer.
pub fn detectProcessLimits() -> OsProcessLimits
{
  let mut out: OsProcessLimits = OsProcessLimits::default();

  // getrlimit(RLIMIT_NPROC) — soft and hard per-user caps.
  let mut rlim: libc::rlimit = unsafe{ std::mem::zeroed() };
  // SAFETY: getrlimit writes into a valid `rlimit` pointer; the syscall
  // itself cannot fail in a way that would leave `rlim` half-initialized
  // (it either fills both fields or returns -1). We still zero-init so a
  // hypothetical partial-write is observable as `0` rather than garbage.
  let rc: i32 = unsafe{ libc::getrlimit(libc::RLIMIT_NPROC, &mut rlim) };
  if rc == 0
  {
    let soft: u64 = rlim.rlim_cur;
    let hard: u64 = rlim.rlim_max;
    // `RLIM_INFINITY` is `u64::MAX` on Linux. Treat "infinity" as "no cap
    // exposed" — the real bound then comes from `kernel.threads-max`.
    if soft != u64::MAX { out.userSoft = Some(soft); }
    if hard != u64::MAX { out.userHard = Some(hard); }
  }

  out.systemHard = readSystemThreadCap();
  out
}

// ==============================================================================================

/// Reads the system-wide cap on threads/processes.
///
/// Linux: `/proc/sys/kernel/threads-max` — a single integer in ASCII,
/// readable without privilege. Missing on non-Linux Unix; treated as `None`.
#[cfg(target_os = "linux")]
fn readSystemThreadCap() -> Option<u64>
{
  let bytes: Vec<u8> = std::fs::read("/proc/sys/kernel/threads-max").ok()?;
  let text: &str = std::str::from_utf8(&bytes).ok()?;
  let trimmed: &str = text.trim();
  trimmed.parse::<u64>().ok()
}

/// Reads the system-wide cap on processes via `sysctl kern.maxproc`.
///
/// `kern.maxproc` is the per-system limit `launchctl maxproc` exposes; it
/// is readable by any process. Missing on systems where `sysctl` does not
/// expose this name (older Darwin, embedded); treated as `None`.
#[cfg(target_os = "macos")]
fn readSystemThreadCap() -> Option<u64>
{
  // SAFETY: `sysctl` with `KERN_MAXPROC` writes into a caller-provided
  // integer buffer. We ask for one `c_int` (the historical type of
  // `kern.maxproc`); if the kernel returns a different size, we ignore it.
  let mut name: [i32; 2] = [libc::CTL_KERN, libc::KERN_MAXPROC];
  let mut value: libc::c_int = 0;
  let mut size: libc::size_t = std::mem::size_of::<libc::c_int>();
  let rc: i32 = unsafe{
    libc::sysctl(
      name.as_mut_ptr(),
      2,
      &mut value as *mut _ as *mut libc::c_void,
      &mut size,
      std::ptr::null_mut(),
      0
    )
  };
  if rc == 0 { Some(value as u64) } else { None }
}

/// Non-Linux, non-macOS Unix: no system-wide thread cap is exposed through
/// a portable interface. The user-level `RLIMIT_NPROC` (read by
/// [`detectProcessLimits`]) is the only bound we know.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn readSystemThreadCap() -> Option<u64> { None }

// =================================================================================================
