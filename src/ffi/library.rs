use parking_lot::RwLockReadGuard;
use parking_lot::RwLock;
use crate::pathResolver::resolveGlobal;
use crate::ffi::scope;
use std::cell::RefMut;
use crate::ffi::errors::FFIError;
use fxhash::FxHashMap;
use crate::zygote::ClonedZygote;
use crate::zygote::ZygoteState;
use crate::ffi::value::{Type, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{OnceLock};
use crate::zygote::{FFIRequest, FFIResponse, ZygoteStack};
// =================================================================================================

/// Counter for assigning unique identifiers to libraries.
static NextLibraryID: AtomicUsize = AtomicUsize::new(1);
/// Global registry of loaded libraries by their identifiers.
static RegisteredLibraries: OnceLock<RwLock<FxHashMap<usize, String>>> = OnceLock::new();

/// Returns the next unique library identifier.
#[inline(always)]
fn nextLibraryId() -> usize
{
  NextLibraryID.fetch_add(1, Ordering::SeqCst)
}

/// Returns the global registry of registered libraries.
#[inline(always)]
fn getRegistry() -> &'static RwLock<FxHashMap<usize, String>>
{
  RegisteredLibraries.get_or_init(|| RwLock::new(FxHashMap::default()))
}

/// Adds a library to the registry by its identifier.
#[inline]
fn registerLibrary(id: usize, path: &str) -> ()
{
  getRegistry().write().insert(id, path.to_string());
}

/// Removes a library from the registry by its identifier.
#[inline]

fn unregisterLibrary(id: usize) -> ()
{
  getRegistry().write().remove(&id);
}

// =================================================================================================

/// Sends a raw FFI request to the active zygote clone in the current thread's stack.
pub(super) fn sendRawRequest(request: FFIRequest) -> Result<Value, FFIError>
{
  // Check whether the global zygote in ZygoteState has been initialized
  if ZygoteState.get().is_none() {
    // todo For callById this will be a repeated check.
    //  But in callById it is better to check it immediately.
    return Err(FFIError::ZygoteNotInitialized);
  }
  
  // Search for the active clone in the local stack of the current thread
  ZygoteStack.with(|stack| {
    let mut mutStack: RefMut<Vec<ClonedZygote>> = stack.borrow_mut();

    // If the stack is empty — it means the call is being made outside the context of ffi!{}
    let zygote: &mut ClonedZygote = mutStack
      .last_mut()
      .ok_or(FFIError::NoActiveZygoteScope)?;

    // Execute the FFI request through the current zygote
    match zygote.call(request) {
      Ok(FFIResponse::Ok(val)) => Ok(val),
      Ok(FFIResponse::Err(err)) => Err(err),
      Err(err) => Err(FFIError::ZygoteCommunicationFailed(err))
    }
  })
}

/// Performs an FFI function call by the identifier of the registered library.
fn callById(
  libraryId: usize,
  libraryPath: &str,
  functionName: &str,
  args: Vec<Value>,
  resultType: Type
) -> Result<Value, FFIError> 
{
  // Check whether the global zygote in ZygoteState has been initialized
  if ZygoteState.get().is_none() {
    return Err(FFIError::ZygoteNotInitialized);
  }

  // Retrieve the path to the `.so` from the registry and construct an FFIRequest
  let registry: RwLockReadGuard<FxHashMap<usize, String>> = getRegistry().read();
  if !registry.contains_key(&libraryId) {
     return Err(FFIError::LibraryNotFound{ libraryPath: libraryPath.to_string() });
  }
  drop(registry);
  
  sendRawRequest(FFIRequest::Call {
    libraryPath: libraryPath.to_string(), functionName: functionName.to_string(), args, resultType,
  })
}

// =================================================================================================

/// Handle of the loaded library with a restriction on available methods.
#[doc(hidden)]
pub struct __Library<const Allowed: bool = false>
{
  /// Library identifier
  libraryId: usize,
  /// Path to the loaded library
  libraryPath: String
}

// Methods that are always available
impl<const Allowed: bool> __Library<Allowed>
{
  /// Returns the library identifier
  #[inline(always)]
  pub const fn id(&self) -> usize
  {
    self.libraryId
  }
}

impl<const Allowed: bool> Drop for __Library<Allowed> 
{
  /// Manual or automatic deletion
  fn drop(&mut self) {
    unregisterLibrary(self.libraryId)
  }
}

// Methods available only inside ffi!{}
impl __Library<true>
{
  /// Executes a function call from the loaded library.
  pub fn call(
    &self,
    functionName: &str,
    args: Vec<Value>,
    resultType: Type,
  ) -> Result<Value, FFIError>
  {
    callById(self.libraryId, &self.libraryPath, functionName, args, resultType)
  }

  /// Loads the library and registers it for further calls.
  pub fn load(libraryPath: &str) -> Result<Self, FFIError>
  {
    let resolved: String = scope::resolveViaScope(libraryPath)
      .or_else(|| resolveGlobal(libraryPath))
      .unwrap_or_else(|| libraryPath.to_string());

    let libraryId: usize = nextLibraryId();
    registerLibrary(libraryId, &resolved);
    Ok(Self{ libraryId, libraryPath: resolved })
  }
  
  /// Unloads the library and removes it from the registry;
  ///
  /// Here self instead of &self is used so that after removal it is not possible
  /// to use the library further. The compiler sees this.
  pub fn unload(self) -> Result<(), FFIError>
  {
    // Do nothing: at the end of the function self will be dropped,
    // and the Drop implementation will be triggered, 
    // which will call unregisterLibrary() itself.
    Ok(())
  }
}

/// Public type from the outside.
pub type Library = __Library<false>;

/// Hidden type for ffi!
#[doc(hidden)]
pub type __FFILibrary = __Library<true>;

// =================================================================================================

#[cfg(test)]
mod tests
{
  use crate::ffi;
  use crate::ffi::library::getRegistry;
  use crate::ffi::value::Value;
  use crate::ffi::value::Type;
  // ===============================================================================================

  /// Checks that library is removed from registry when explicitly dropped.
  #[test]
  fn libraryDrop() -> ()
  {
    let id: usize = ffi!{
      let libm: Library = Library::load("libm.so.6")?;
      let id: usize = libm.id();
      drop(libm);
      Ok(id)
    }.expect("ffi block failed");

    assert!(!getRegistry().read().contains_key(&id));
  }

  /// Checks that library is removed from registry 
  /// when automatically dropped on scope exit.
  #[test]
  fn libraryAutoDrop() -> ()
  {
    let id: usize = ffi!{
      let libm: Library = Library::load("libm.so.6")?;
      let id: usize = libm.id();
      Ok(id)
    }.expect("ffi block failed");

    assert!(!getRegistry().read().contains_key(&id));
  }

  /// Checks that library is removed from registry 
  /// when unloaded via `unload()`.
  #[test]
  fn libraryUnload() -> ()
  {
    let id: usize = ffi!{
      let libm: Library = Library::load("libm.so.6")?;
      let id: usize = libm.id();
      libm.unload()?;
      Ok(id)
    }.expect("ffi block failed");

    assert!(!getRegistry().read().contains_key(&id));
  }

  // ===============================================================================================

  /// Checks calling the sqrt function from the libm library.
  #[test]
  fn sqrt() -> ()
  {
    let result: Value = ffi!{
      let libm: Library = Library::load("libm.so.6")?;
      let args: Vec<Value> = vec![Value::F64(4.0)];
      Ok(libm.call("sqrt", args, Type::F64)?)
    }.expect("FFI call failed");

    if let Value::F64(val) = result {
      assert!((val - 2.0).abs() < f64::EPSILON);
    } else {
      panic!("Expected F64");
    }
  }

  /// Checks calling the abs function from the libm library.
  #[test]
  fn abs() -> ()
  {
    let result: Value = ffi!{
      let libm: Library = Library::load("libm.so.6")?;
      let args: Vec<Value> = vec![Value::I32(-5)];
      Ok(libm.call("abs", args, Type::I32)?)
    }.expect("FFI call failed");

    if let Value::I32(val) = result {
      assert_eq!(val, 5);
    } else {
      panic!("Expected I32");
    }
  }

  // ===============================================================================================

  /// Checks repeated calls inside a single ffi!{} - uses cached dlopen.
  #[test]
  fn multipleCallsInSingleLibrary() -> ()
  {
    let results: Vec<Value> = ffi!{
      let mut outputs: Vec<Value> = Vec::with_capacity(10);
      let libm: Library = Library::load("libm.so.6")?;
  
      // 10 consecutive libm.call() calls with a single loaded library
      for i in 1..=10 
      {
        let input: f64 = (i * i) as f64;
        let args: Vec<Value> = vec![Value::F64(input)];
        let res: Value = libm.call("sqrt", args, Type::F64)?;
        outputs.push(res);
      }
  
      Ok(outputs)
    }.expect("Batch FFI call failed");

    assert_eq!(results.len(), 10);

    for (i, val) in results.into_iter().enumerate()
    {
      let expected: f64 = (i + 1) as f64;
      if let Value::F64(actual) = val {
        assert!((actual - expected).abs() < f64::EPSILON, "Expected {}, got {}", expected, actual);
      } else {
        panic!("Expected Value::F64 at index {}", i);
      }
    }
  }

  // ===============================================================================================

  /// Checks passing Value::None as an argument - should return an error.
  #[test]
  fn noneArgumentFails() -> ()
  {
    let result: Result<Value, _> = ffi!{
      let libm: Library = Library::load("libm.so.6")?;
      let args: Vec<Value> = vec![Value::None];
      let res: Value = libm.call("sqrt", args, Type::F64)?;
      Ok(res)
    };

    assert!(result.is_err(), "FFI call with Value::None should fail");
  }

  // ===============================================================================================
}

// =================================================================================================