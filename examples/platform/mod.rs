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

// =================================================================================================

/// Linux → ".so", macOS → ".dylib"
///
/// todo By the way, in theory it could have been a public helper,
///  because something like this can be convenient in multi-platform code.
#[allow(unused_macros)]
macro_rules! platformExt
{
  ($path:literal) => {{
    #[cfg(target_os = "macos")]
    { concat!($path, ".dylib") }
    #[cfg(not(target_os = "macos"))]
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
}

// =================================================================================================
