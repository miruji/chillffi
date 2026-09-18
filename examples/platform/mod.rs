// This is a common data storage for examples and tests at the same time.
// There is no main.rs here, so it is ignored as an example.
// Platform-dependent data can be stored here.

// =================================================================================================

#[cfg(target_os = "linux")]
#[allow(unused_imports)]
pub use linux::*;

#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use macos::*;

#[cfg(windows)]
#[allow(unused_imports)]
pub use windows::*;

// =================================================================================================

/// Linux → ".so", macOS → ".dylib", Windows → ".dll"
///
/// todo By the way, in theory it could have been a public helper,
///  because something like this can be convenient in multi-platform code.
#[allow(unused_macros)]
macro_rules! platformExt
{
  ($path:literal) => {{
    #[cfg(target_os = "macos")]
    { concat!($path, ".dylib") }
    #[cfg(windows)]
    { concat!($path, ".dll") }
    #[cfg(not(any(target_os = "macos", windows)))]
    { concat!($path, ".so") }
  }};
}
#[allow(unused_imports)]
pub(crate) use platformExt;

// =================================================================================================

#[cfg(target_os = "linux")]
mod linux
{
  //
  #[allow(dead_code)]
  pub const LibcPath: &str = "libc.so.6";
  #[allow(dead_code)]
  pub const LibmPath: &str = "libm.so.6";

  // stat
  #[allow(dead_code)]
  pub const StatSize: usize = 144;
  #[allow(dead_code)]
  pub const StSizeOffset: usize = 48;
  #[allow(dead_code)]
  pub const EtcHostnameString: &str = "/etc/hostname";
  #[allow(dead_code)]
  pub const EtcHostnameCString: &std::ffi::CStr = c"/etc/hostname";
  #[allow(dead_code)]
  pub const StatSymbolName: &str = "stat";

  #[allow(dead_code)]
  pub const OpenSymbolName: &str = "open";
  #[allow(dead_code)]
  pub const CloseSymbolName: &str = "close";
  #[allow(dead_code)]
  pub const ReadSymbolName: &str = "read";
  #[allow(dead_code)]
  pub const WriteSymbolName: &str = "write";
  #[allow(dead_code)]
  pub const PipeSymbolName: &str = "pipe";
  #[allow(dead_code)]
  pub const StrdupSymbolName: &str = "strdup";

  #[allow(dead_code)]
  pub const TimeLibPath: &str = LibcPath;
  #[allow(dead_code)]
  pub const TimeSymbolName: &str = "clock_gettime";

  #[allow(dead_code)]
  pub const SignalNumber: i32 = 10; // SIGUSR1
}

// =================================================================================================

#[cfg(target_os = "macos")]
mod macos
{
  //
  #[allow(dead_code)]
  pub const LibcPath: &str = "libSystem.dylib";
  #[allow(dead_code)]
  pub const LibmPath: &str = "libSystem.dylib";

  // stat
  #[allow(dead_code)]
  pub const StatSize: usize = 144;
  #[allow(dead_code)]
  pub const StSizeOffset: usize = 96;
  #[allow(dead_code)]
  pub const EtcHostnameString: &str = "/etc/hosts";
  #[allow(dead_code)]
  pub const EtcHostnameCString: &std::ffi::CStr = c"/etc/hosts";
  #[cfg(target_arch = "aarch64")]
  #[allow(dead_code)]
  pub const StatSymbolName: &str = "stat";
  #[cfg(target_arch = "x86_64")]
  #[allow(dead_code)]
  pub const StatSymbolName: &str = "stat$INODE64"; // Important: on Intel you need to explicitly use the 64-bit version.

  #[allow(dead_code)]
  pub const OpenSymbolName: &str = "open";
  #[allow(dead_code)]
  pub const CloseSymbolName: &str = "close";
  #[allow(dead_code)]
  pub const ReadSymbolName: &str = "read";
  #[allow(dead_code)]
  pub const WriteSymbolName: &str = "write";
  #[allow(dead_code)]
  pub const PipeSymbolName: &str = "pipe";
  #[allow(dead_code)]
  pub const StrdupSymbolName: &str = "strdup";

  #[allow(dead_code)]
  pub const TimeLibPath: &str = LibcPath;
  #[allow(dead_code)]
  pub const TimeSymbolName: &str = "clock_gettime";

  #[allow(dead_code)]
  pub const SignalNumber: i32 = 10; // SIGUSR1
}

// =================================================================================================

#[cfg(windows)]
mod windows
{
  // ucrtbase.dll is the CRT the MSVC Rust toolchain itself links against,
  // so errno observed through chillffi and errno set by these functions
  // are the same instance. No separate math library: libm's contents
  // (sqrt, pow, ...) are exported from the same DLL.
  #[allow(dead_code)]
  pub const LibcPath: &str = "ucrtbase.dll";
  #[allow(dead_code)]
  pub const LibmPath: &str = "ucrtbase.dll";

  // struct _stat64, 56 bytes; st_size at offset 24.
  #[allow(dead_code)]
  pub const StatSize: usize = 56;
  #[allow(dead_code)]
  pub const StSizeOffset: usize = 24;
  #[allow(dead_code)]
  pub const EtcHostnameString: &str = "C:\\Windows\\System32\\drivers\\etc\\hosts";
  #[allow(dead_code)]
  pub const EtcHostnameCString: &std::ffi::CStr = c"C:\\Windows\\System32\\drivers\\etc\\hosts";
  #[allow(dead_code)]
  pub const StatSymbolName: &str = "_stat64";

  // The UCRT exports the POSIX-shaped calls under underscored names.
  #[allow(dead_code)]
  pub const OpenSymbolName: &str = "_open";
  #[allow(dead_code)]
  pub const CloseSymbolName: &str = "_close";
  #[allow(dead_code)]
  pub const ReadSymbolName: &str = "_read";
  #[allow(dead_code)]
  pub const WriteSymbolName: &str = "_write";
  #[allow(dead_code)]
  pub const PipeSymbolName: &str = "_pipe";
  #[allow(dead_code)]
  pub const StrdupSymbolName: &str = "_strdup";

  // No clock_gettime in the UCRT; GetSystemTimeAsFileTime writes a FILETIME
  // (u64 ticks, 100ns since 1601-01-01) instead of a two-field timespec.
  #[allow(dead_code)]
  pub const TimeLibPath: &str = "kernel32.dll";
  #[allow(dead_code)]
  pub const TimeSymbolName: &str = "GetSystemTimeAsFileTime";
  /// Ticks between the FILETIME epoch (1601-01-01) and the Unix epoch.
  #[allow(dead_code)]
  pub const FileTimeUnixEpoch: u64 = 116_444_736_000_000_000;

  // No SIGUSR1 here; the UCRT accepts SIGTERM.
  #[allow(dead_code)]
  pub const SignalNumber: i32 = 15; // SIGTERM
}

// =================================================================================================
