use crate::ffi::types::Type;
use crate::ffi::types::primitive::Primitive;
use crate::ffi::types::primitive::FfiArg;
use crate::ffi::types::Value;
use std::cell::RefMut;
use std::marker::PhantomData;
use parking_lot::RwLockReadGuard;
use parking_lot::RwLock;
use crate::ffi::errors::FFIError;
use fxhash::FxHashMap;
use crate::zygote::ZygoteState;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{OnceLock};
use crate::__ffiInternal::ClonedZygote;
use crate::zygote::{FFIRequest, FFIResponse, ZygoteStack};
// =================================================================================================

/// Counter for assigning unique identifiers to libraries.
static NextLibraryID: AtomicUsize = AtomicUsize::new(1);
/// Global registry of loaded libraries by their identifiers.
static RegisteredLibraries: OnceLock<RwLock<FxHashMap<usize, String>>> = OnceLock::new();

/// Returns the next unique library identifier.
#[inline(always)]
pub(super) fn nextLibraryId() -> usize
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
pub(super) fn registerLibrary(id: usize, path: &str) -> ()
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
  // Check whether the global zygote in ZygoteState has been initialized.
  if ZygoteState.get().is_none() {
    // todo For callById this will be a repeated check.
    //  But in callById it is better to check it immediately.
    return Err(FFIError::ZygoteNotInitialized);
  }

  // Retrieve the most recently pushed active zygote 
  // from the thread-local stack to execute the raw FFI request.
  ZygoteStack.with(|stack| {
    let mut mutStack: RefMut<Vec<ClonedZygote>> = stack.borrow_mut();
    let zygote: &mut ClonedZygote = mutStack.last_mut().ok_or(FFIError::NoActiveZygoteScope)?;

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
  // Check whether the global zygote in ZygoteState has been initialized.
  if ZygoteState.get().is_none() {
    return Err(FFIError::ZygoteNotInitialized);
  }

  // Retrieve the path to the library from the registry and construct an FFIRequest.
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

/// Handle of a loaded library, bound to the [`Scope<'g>`] it was loaded through —
/// same model as [`AllocatedMemory<'g>`](crate::ffi::allocatedMemory::AllocatedMemory).
///
/// `Library<'g>` cannot outlive the scope that created it: there is no way to
/// construct one except via [`Scope::load`](crate::ffi::scope::Scope::load),
/// which requires a live `Scope<'g>` in the first place. This replaces the old
/// `__Library<const Allowed: bool>` gate — the lifetime *is* the gate now.
pub struct Library<'g>
{
  /// Library identifier.
  libraryId: usize,
  /// Path to the loaded library.
  libraryPath: String,
  /// Phantom lifetime marker tying the handle to the scope it was loaded through.
  _scope: PhantomData<&'g ()>
}

impl<'g> Library<'g>
{
  /// Creates a handle for an already-registered library. Only callable from
  /// [`Scope::load`](crate::ffi::scope::Scope::load) — `libraryId` must come
  /// from [`nextLibraryId`] and already be registered via [`registerLibrary`].
  #[inline(always)]
  pub(super) const fn new(libraryId: usize, libraryPath: String) -> Self
  {
    Self { libraryId, libraryPath, _scope: PhantomData }
  }

  /// Returns the library identifier.
  #[inline(always)]
  pub const fn id(&self) -> usize
  {
    self.libraryId
  }

  /// Returns the resolved path the library was loaded from.
  #[inline(always)]
  pub fn path(&self) -> &str
  {
    &self.libraryPath
  }
}

impl<'g> Drop for Library<'g>
{
  /// Manual or automatic deletion.
  fn drop(&mut self) {
    unregisterLibrary(self.libraryId)
  }
}

// =================================================================================================

/// Builder for fluent FFI calls.
#[doc(hidden)]
pub struct CallBuilder<'a, 'g> 
{
  lib: &'a Library<'g>,
  name: String,
  args: Vec<Value>,
}

impl<'a, 'g> CallBuilder<'a, 'g>
{
  #[inline]
  pub fn new(lib: &'a Library<'g>, name: &str) -> Self 
  {
    Self {
      lib,
      name: name.to_string(),
      args: Vec::new(),
    }
  }

  /// Append one argument. Chainable.
  #[inline]
  pub fn arg<T: FfiArg>(mut self, arg: T) -> Self
  {
    self.args.push(arg.intoFfiValue());
    self
  }

  /// Finalize: execute and return a typed result.
  #[inline]
  pub fn result<T: Primitive>(self) -> Result<T, FFIError>
  {
    self.lib.__call(&self.name, self.args)
  }

  /// Finalize: execute and discard the result (void / fire-and-forget).
  #[inline]
  pub fn void(self) -> Result<(), FFIError>
  {
    self.lib.__call::<()>(&self.name, self.args).map(|_| ())
  }
}

// =================================================================================================

impl<'g> Library<'g> 
{
  /// Starts a fluent call builder.
  #[inline]
  pub fn call(&self, name: &str) -> CallBuilder<'_, 'g> 
  {
    CallBuilder::new(self, name)
  }

  /// Executes a function call from the loaded library.
  ///
  /// todo It should be completely hidden and not work directly
  #[inline]
  #[doc(hidden)]
  pub fn __call<T: Primitive>(&self, functionName: &str, args: Vec<Value>) -> Result<T, FFIError>
  {
    let raw: Value = callById(self.libraryId, &self.libraryPath, functionName, args, T::TypeTag)?;
    T::fromValue(raw)
  }

  /// Fire-and-forget variant: a call without waiting for or typing the result.
  ///
  /// todo It should be completely hidden and not work directly
  #[inline]
  #[doc(hidden)]
  pub fn __callv(&self, functionName: &str, args: Vec<Value>) -> Result<(), FFIError>
  {
    self.__call::<()>(functionName, args)
  }

  // There is no variant with `let a = call(`. Because you either expect void, or specify the type.
  // It would be rough to require a different type specification if you can do it directly in `let a:`.

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

// =================================================================================================

#[cfg(test)]
mod tests
{
  use crate::ffi;
  use crate::ffi::library::getRegistry;
  // ===============================================================================================

  /// Checks that library is removed from registry when explicitly dropped.
  #[test]
  fn libraryDrop() -> ()
  {
    let id: usize = ffi!(|scope| {
      let libm: Library = scope.load("libm.so.6")?;
      let id: usize = libm.id();
      drop(libm);
      Ok(id)
    }).expect("ffi block failed");

    assert!(!getRegistry().read().contains_key(&id));
  }

  /// Checks that library is removed from registry 
  /// when automatically dropped on scope exit.
  #[test]
  fn libraryAutoDrop() -> ()
  {
    let id: usize = ffi!(|scope| {
      let libm: Library = scope.load("libm.so.6")?;
      let id: usize = libm.id();
      Ok(id)
    }).expect("ffi block failed");

    assert!(!getRegistry().read().contains_key(&id));
  }

  /// Checks that library is removed from registry 
  /// when unloaded via [`unload()`].
  #[test]
  fn libraryUnload() -> ()
  {
    let id: usize = ffi!(|scope| {
      let libm: Library = scope.load("libm.so.6")?;
      let id: usize = libm.id();
      libm.unload()?;
      Ok(id)
    }).expect("ffi block failed");

    assert!(!getRegistry().read().contains_key(&id));
  }

  // ===============================================================================================

  /// Checks calling the sqrt function from the libm library.
  #[test]
  fn sqrt() -> ()
  {
    let result: f64 = ffi!(|scope| {
      let libm: Library = scope.load("libm.so.6")?;
      Ok( libm.call("sqrt").arg::<f64>(4.0).result()? )
    }).expect("FFI call failed");

    assert!((result - 2.0).abs() < f64::EPSILON);
  }

  /// Checks calling the abs function from the libm library.
  #[test]
  fn abs() -> ()
  {
    let result: i32 = ffi!(|scope| {
      let libm: Library = scope.load("libm.so.6")?;
      Ok( libm.call("abs").arg::<i32>(-5).result()? )
    }).expect("FFI call failed");

    assert_eq!(result, 5);
  }

  // ===============================================================================================

  /// Checks repeated calls inside a single [`ffi!`] - uses cached dlopen.
  #[test]
  fn multipleCallsInSingleLibrary() -> ()
  {
    let results: Vec<f64> = ffi!(|scope| {
      let mut outputs: Vec<f64> = Vec::with_capacity(10);
      let libm: Library = scope.load("libm.so.6")?;
  
      // 10 consecutive calls with a single loaded library
      for i in 1..=10 
      {
        let input: f64 = (i * i) as f64;
        let res: f64 = libm.call("sqrt").arg(input).result()?;
        outputs.push(res);
      }
  
      Ok(outputs)
    }).expect("Batch FFI call failed");

    assert_eq!(results.len(), 10);

    for (i, val) in results.into_iter().enumerate()
    {
      let expected: f64 = (i + 1) as f64;
      assert!((val - expected).abs() < f64::EPSILON, "Expected {}, got {}", expected, val);
    }
  }

  // ===============================================================================================
}

// =================================================================================================