#[cfg(not(target_os = "linux"))]
compile_error!("chillffi supports only Linux operating systems.");
// Currently available only on Linux, although it should work on UNIX in general.
// But I have not tested it on macOS.

// =================================================================================================

pub mod worker;
pub mod zygote;
pub mod ffi;

// =================================================================================================

use std::{env};
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

  // Do this once to start the main zygote
  initZygote().expect("Failed to setup zygote");
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
  // Вариант с доступом к Scope. `ffi!(|scope| {})`. 
  // Может быть любое имя scope - главное без повторений внутри {}.
  // Scope<'g> заимствует ScopeGuard этого блока, поэтому AllocatedMemory<'g>
  // не может быть возвращена наружу — это ловит компилятор, а не мы.
  (|$scopeName:ident| { $($body:tt)* }) => 
  {
    (|| -> Result<_, $crate::ffi::errors::FFIError> 
    {
      #[allow(unused_imports)]
      use $crate::ffi::library::__FFILibrary as Library;
 
      // Creating a clone-zygote from the main one
      let zygote = $crate::zygote::ClonedZygote::getMeClone()?;
 
      // Registering the clone-zygote in the current thread's ZygoteStack
      let _guard = $crate::zygote::ZygoteGuard::enter(zygote);
 
      // ScopeGuard живёт строго в границах этого блока; $scopeName заимствует его.
      let _scopeGuard = $crate::ffi::scope::ScopeGuard::new();
      let $scopeName = $crate::ffi::scope::Scope::new(&_scopeGuard);
 
      // Executing the body
      $($body)*
    })()
  };
 
  // Вариант без Scope — если alloc() не нужен, ScopeGuard вообще не создаётся.
  ($($body:tt)*) => 
  {
    (|| -> Result<_, $crate::ffi::errors::FFIError> 
    {
      #[allow(unused_imports)]
      use $crate::ffi::library::__FFILibrary as Library;
 
      // Creating a clone-zygote from the main one
      let zygote = $crate::zygote::ClonedZygote::getMeClone()?;
 
      // Registering the clone-zygote in the current thread's ZygoteStack
      let _guard = $crate::zygote::ZygoteGuard::enter(zygote);
 
      // Executing the body
      $($body)*
    })()
  };

}

// =================================================================================================