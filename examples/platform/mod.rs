// Это общее хранилище данных для примеров.
// Т.е. тут нет main.rs - оно игнорируется.

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
  pub const StatSymbolName: &str = "stat$INODE64"; // Важно: на Intel нужно явно брать 64-bit версию
}

//
#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use macos::*;