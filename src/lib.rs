#[cfg(not(target_os = "linux"))]
compile_error!("chillffi supports only Linux operating systems.");
// Currently available only on Linux, although it should work on UNIX in general.
// But I have not tested it on macOS.

// =================================================================================================

pub mod worker;
pub mod zygote;
pub mod ffi;

// =================================================================================================

use std::{env, io};
use crate::zygote::{initZygote, runAsZygote, ZygoteFlag};

// =================================================================================================

/// Single entry point for zygote initialization in any binary (including tests).
/// Checks whether the process is running as a zygote; if so — switches to daemon mode,
/// otherwise — initializes the parent side.
#[ctor::ctor(unsafe)]
fn zygoteEntrypoint() -> ()
{
  let mut args = env::args_os();
  args.next();
  if let Some(arg) = args.next() 
  {
    if arg == ZygoteFlag 
    {
      runAsZygote();
    }
  }
  //
}

/// Zygote initialization; call once, 
/// as the very first line of the normal main().
pub fn setupZygote() -> io::Result<()>
{
  initZygote()
}

// =================================================================================================

/// Main macro for working with FFI.
///
/// It creates a copy of the zygote from the main zygote.
///
/// After that, any FFI code can be executed inside it.
///
/// Library specifically blocks FFI calls outside this macro.
///
/// Isolation allows adding FFI insertions without breaking or corrupting the main runtime.
///
/// todo
///  Important: It will take ownership of the Library type — therefore, the code inside
///  will have to specify it differently when this data type matches. However, this will
///  be quite rare, because FFI insertions should be rare and it is not guaranteed that
///  exactly Library will end up there.
///  The simplest solution would be for the user to rename the type — then they will not
///  see errors for their Library type.
#[macro_export]
macro_rules! ffi 
{
  ($($body:tt)*) => 
  {
    (|| -> Result<_, $crate::ffi::library::FFIError> 
    {
      use $crate::ffi::library::__FFILibrary as Library;
      use $crate::zygote::ClonedZygote;
      use $crate::zygote::ZygoteGuard;

      // Creating a clone-zygote from the main one
      let zygote: ClonedZygote = ClonedZygote::getMeClone()
        .map_err(|e| $crate::ffi::library::FFIError::Other(e.to_string()))?;
      
      // Registering the clone-zygote in the current thread's ZygoteStack
      let _guard: ZygoteGuard = ZygoteGuard::enter(zygote);

      // Executing the body
      $($body)*
    })()
  };
}

// =================================================================================================

#[cfg(test)]
pub mod tests 
{
  use std::sync::Once;
  // ===============================================================================================
  
  static Init: Once = Once::new();

  /// Initializes the FFI environment once before running tests.
  ///
  /// Uses Once for a safe one-time execution of setupZygote().
  pub fn setup() 
  {
    Init.call_once(|| {
      super::setupZygote().expect("Failed to setup zygote");
    });
  }

  // ===============================================================================================
}

// =================================================================================================