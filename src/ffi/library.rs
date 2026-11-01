use crate::__ffiInternal::ClonedZygote;
use crate::errnoPolicy::globalReadErrno;
use crate::ffi::errors::FFIError;
use crate::ffi::scope::currentScopeReadErrno;
use crate::ffi::types::primitive::{Arg, FfiArg, FfiPrimitive, StructValue};
use crate::ffi::types::Type;
use crate::ffi::types::Value;
use crate::zygote::ZygoteState;
use crate::zygote::{FFIRequest, FFIResponse, ZygoteStack};
use fxhash::FxHashMap;
use parking_lot::RwLock;
use parking_lot::RwLockReadGuard;
use std::cell::RefMut;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
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

thread_local!{
  /// errno captured by the most recently completed request, if the request
  /// asked for it (see [`FFIRequest::Call`]/[`FFIRequest::CallPointer`]).
  /// Overwritten by every `sendRawRequest` call, including non-Call ones —
  /// so it always reflects "immediately after the last thing that ran",
  /// which is what a caller checking it right after a suspicious result wants.
  static LastErrno: std::cell::Cell<Option<i32>> = const {
    std::cell::Cell::new(None) 
  };

  /// Windows counterpart of `LastErrno`: `GetLastError`, `None` elsewhere.
  static LastOsError: std::cell::Cell<Option<u32>> = const {
    std::cell::Cell::new(None)
  };
}

/// Reads the errno left behind by the most recent request on this thread.
/// See [`crate::ffi::scope::Scope::lastErrno`] — the public entry point.
pub(super) fn lastErrno() -> Option<i32>
{
  LastErrno.get()
}

/// See [`crate::ffi::scope::Scope::lastOsError`] — the public entry point.
pub(super) fn lastOsError() -> Option<u32>
{
  LastOsError.get()
}

/// Resolves the effective errno-capture flag for a call: an explicit
/// per-call override wins, then the enclosing scope's override (see
/// [`Scope::setReadErrno`](crate::ffi::scope::Scope::setReadErrno)), then the
/// global default (see [`crate::errnoPolicy::setGlobalReadErrno`]) — same
/// most-specific-wins order as [`Scope::load`](crate::ffi::scope::Scope::load)'s
/// path resolution.
#[inline]
pub(super) fn resolveReadErrno(perCall: Option<bool>) -> bool
{
  perCall
    .unwrap_or_else(|| currentScopeReadErrno()
      .unwrap_or_else(globalReadErrno))
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
      Ok(FFIResponse::Ok(val, errno, osError)) => {
        LastErrno.set(errno);
        LastOsError.set(osError);
        Ok(val)
      }
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
  resultType: Type,
  readErrno: bool,
  fixedArgs: Option<usize>
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
    libraryPath: libraryPath.to_string(),
    functionName: functionName.to_string(),
    args,
    resultType,
    readErrno,
    fixedArgs
  })
}

// =================================================================================================

/// Handle of a loaded library, bound to the `Scope<'g>` it was loaded through —
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
  /// The library against which the call is issued.
  lib: &'a Library<'g>,

  /// Name of the function to look up and call.
  name: String,

  /// Arguments collected for the call, in order.
  args: Vec<Value>,

  /// Per-call override of errno capture. `None` falls through to the
  /// enclosing scope's setting, then the global default — see [`resolveReadErrno`].
  readErrno: Option<bool>
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
      readErrno: None
    }
  }

  /// Append one argument. Chainable.
  #[inline]
  pub fn arg<T: FfiArg>(mut self, arg: T) -> Self
  {
    self.args.push(arg.intoFfiValue().0);
    self
  }

  /// Forces errno capture for this call specifically, regardless of the
  /// scope's or global default. Read it back afterward via
  /// [`Scope::lastErrno`](crate::ffi::scope::Scope::lastErrno).
  #[inline]
  pub const fn errno(mut self) -> Self
  {
    self.readErrno = Some(true);
    self
  }

  /// Forces errno capture *off* for this call, overriding a scope/global
  /// default that would otherwise have enabled it.
  #[inline]
  pub const fn noErrno(mut self) -> Self
  {
    self.readErrno = Some(false);
    self
  }

  /// Switches the chain into variadic mode — the exact analogue of C's `...`:
  /// every argument added *before* this call is a fixed argument, everything
  /// added *after* is variadic. Returns a [`VariadicCallBuilder`], so the
  /// fixed/variadic boundary is enforced by the type system: the compiler
  /// rejects adding fixed arguments after the variadic part has begun, and
  /// calling `.variadic()` twice on the same chain.
  ///
  /// The number of fixed arguments is frozen at this moment — it equals the
  /// number of `.arg()` calls made so far. libffi's `ffi_prep_cif_var`
  /// requires at least one fixed argument, so a chain that calls
  /// `.variadic()` before any `.arg()` fails with
  /// [`FFIError::BadArgument`] at `.result()`/`.void()` time.
  ///
  /// Note: C applies default argument promotions in the variadic part —
  /// `f32` promotes to `double`, small integer types promote to `int`.
  /// Pass already-promoted types for variadic arguments (`f64` for `%f`,
  /// `i32` for `%d`, ...).
  ///
  /// # Example
  /// ```ignore
  /// // printf(const char *format, ...)
  /// libc.call("printf")
  ///   .arg(c"Hello %s %d\n") // fixed argument (format)
  ///   .variadic()            // <- everything after this is variadic
  ///   .arg(c"world")         // %s
  ///   .arg::<i32>(42)        // %d
  ///   .result::<i32>()?;
  /// ```
  #[inline]
  pub const fn variadic(self) -> VariadicCallBuilder<'a, 'g>
  {
    // The current argument count IS the number of fixed arguments —
    // this is exactly why variadic() consumes self: the boundary is frozen here.
    let fixedArgsCount: usize = self.args.len();
    VariadicCallBuilder { base: self, fixedArgsCount }
  }

  /// Finalize: execute and return a typed result.
  #[inline]
  pub fn result<T: FfiPrimitive>(self) -> Result<T, FFIError>
  {
    let readErrno: bool = resolveReadErrno(self.readErrno);
    self.lib.__call(&self.name, self.args, readErrno, None)
  }

  /// Finalize: execute and return a struct by value with the given field layout.
  ///
  /// The shape is a runtime `&[Type]` list — the same one used by
  /// [`Scope::readDynamicStruct`](crate::ffi::scope::Scope::readDynamicStruct).
  /// Required because the return buffer is untyped until `readStructAt`
  /// decodes it; unlike `.arg(StructValue)`, the result side cannot infer
  /// field types from values that do not exist yet.
  #[inline]
  pub fn resultStruct(self, fields: &[Type]) -> Result<StructValue, FFIError>
  {
    let readErrno: bool = resolveReadErrno(self.readErrno);
    let resultType: Type = Type::structure(fields.iter().cloned());
    let raw: Value = callById(
      self.lib.id(),
      self.lib.path(),
      &self.name,
      self.args,
      resultType,
      readErrno,
      None
    )?;
    match raw {
      Value::Struct(values) => Ok(StructValue::fromValues(values)),
      other => Err(FFIError::Other(format!(
        "expected struct return, got {:?}", other
      )))
    }
  }

  /// Finalize: execute and discard the result (void / fire-and-forget).
  #[inline]
  pub fn void(self) -> Result<(), FFIError>
  {
    let readErrno: bool = resolveReadErrno(self.readErrno);
    self.lib.__call::<()>(&self.name, self.args, readErrno, None).map(|_| ())
  }
}

// =================================================================================================

/// Builder for fluent FFI calls to C-style variadic functions (`...`).
///
/// Produced exclusively by [`CallBuilder::variadic`] — the moment the
/// fixed/variadic boundary is crossed, the chain changes type, so the two
/// argument kinds can never be mixed up: `VariadicCallBuilder` has no
/// `.variadic()` of its own and its `.arg()` appends variadic arguments
/// only. Finalize with `.result()` / `.void()` exactly like [`CallBuilder`].
///
/// Note: C applies default argument promotions in the variadic part —
/// `f32` promotes to `double`, `i8`/`i16` promote to `int`. Pass
/// already-promoted types (`f64`, `i32`, ...) for variadic arguments.
#[doc(hidden)]
pub struct VariadicCallBuilder<'a, 'g>
{
  /// The fixed-argument builder captured at `.variadic()` time.
  base: CallBuilder<'a, 'g>,

  /// Number of fixed arguments: the length of `base.args` at the moment
  /// `.variadic()` was called. libffi's `ffi_prep_cif_var` takes
  /// `nfixedargs` and `ntotalargs` separately, so this travels with the
  /// request as `FFIRequest::Call`'s `fixedArgs`.
  fixedArgsCount: usize
}

impl<'a, 'g> VariadicCallBuilder<'a, 'g>
{
  /// Append one variadic argument. Chainable.
  #[inline]
  pub fn arg<T: FfiArg>(mut self, arg: T) -> Self
  {
    self.base.args.push(arg.intoFfiValue().0);
    self
  }

  /// Forces errno capture for this call specifically, regardless of the
  /// scope's or global default. Read it back afterward via
  /// [`Scope::lastErrno`](crate::ffi::scope::Scope::lastErrno).
  #[inline]
  pub const fn errno(mut self) -> Self
  {
    self.base.readErrno = Some(true);
    self
  }

  /// Forces errno capture *off* for this call, overriding a scope/global
  /// default that would otherwise have enabled it.
  #[inline]
  pub const fn noErrno(mut self) -> Self
  {
    self.base.readErrno = Some(false);
    self
  }

  /// Finalize: execute the variadic call and return a typed result.
  #[inline]
  pub fn result<T: FfiPrimitive>(self) -> Result<T, FFIError>
  {
    let readErrno: bool = resolveReadErrno(self.base.readErrno);
    self.base.lib.__call(&self.base.name, self.base.args, readErrno, Some(self.fixedArgsCount))
  }

  /// Finalize: execute the variadic call and return a struct by value.
  ///
  /// Same contract as [`CallBuilder::resultStruct`] — the field layout must
  /// be supplied explicitly so the return buffer can be decoded.
  #[inline]
  pub fn resultStruct(self, fields: &[Type]) -> Result<StructValue, FFIError>
  {
    let readErrno: bool = resolveReadErrno(self.base.readErrno);
    let resultType: Type = Type::structure(fields.iter().cloned());
    let raw: Value = callById(
      self.base.lib.id(),
      self.base.lib.path(),
      &self.base.name,
      self.base.args,
      resultType,
      readErrno,
      Some(self.fixedArgsCount)
    )?;
    match raw {
      Value::Struct(values) => Ok(StructValue::fromValues(values)),
      other => Err(FFIError::Other(format!(
        "expected struct return, got {:?}", other
      )))
    }
  }

  /// Finalize: execute the variadic call and discard the result (void).
  #[inline]
  pub fn void(self) -> Result<(), FFIError>
  {
    let readErrno: bool = resolveReadErrno(self.base.readErrno);
    self.base.lib.__call::<()>(&self.base.name, self.base.args, readErrno, Some(self.fixedArgsCount))
      .map(|_| ())
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
  pub(crate) fn __call<T: FfiPrimitive>(
    &self,
    functionName: &str,
    args: Vec<Value>,
    readErrno: bool,
    fixedArgs: Option<usize>
  ) -> Result<T, FFIError>
  {
    let raw: Value = callById(
      self.libraryId,
      &self.libraryPath,
      functionName, args,
      T::TypeTag,
      readErrno,
      fixedArgs
    )?;
    T::fromFfiValue(Arg(raw))
  }

  // There is no variant with `let a = call(`. Because you either expect void, or specify the type.
  // It would be rough to require a different type specification if you can do it directly in `let a:`.

  /// Prepares a function for repeated calls: the clone resolves the symbol
  /// once (`dlsym`) and builds the libffi call interface once
  /// (`ffi_prep_cif`), returning a handle; every subsequent
  /// [`Function::call`] skips both steps.
  ///
  /// The signature is pinned here and validated on every call — a mismatch
  /// is [`FFIError::BadArgument`], never UB. The prepared function lives as
  /// long as the enclosing `ffi!` scope (the clone), and dies with it.
  ///
  /// # Example
  /// ```ignore
  /// let sqrt: Function = libm.function("sqrt", &[Type::F64], Type::F64)?;
  /// let r: f64 = sqrt.callTyped(vec![Value::F64(9.0)])?;
  /// ```
  pub fn function(
    &self,
    name: &str,
    argTypes: &[Type],
    resultType: Type
  ) -> Result<Function<'g>, FFIError>
  {
    if ZygoteState.get().is_none() {
      return Err(FFIError::ZygoteNotInitialized);
    }

    let value: Value = sendRawRequest(FFIRequest::PrepareFunction {
      libraryPath: self.libraryPath.clone(),
      functionName: name.to_string(),
      argTypes: argTypes.to_vec(),
      resultType
    })?;

    let Value::Usize(handle) = value else {
      return Err(FFIError::Other(format!(
        "function {name}: unexpected prepare response {value:?}"
      )));
    };

    Ok(Function {
      handle,
      name: name.to_string(),
      argTypes: argTypes.to_vec(),
      _scope: PhantomData
    })
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

// =================================================================================================

/// A prepared function: a handle to a symbol + libffi call interface already
/// resolved inside the clone (see [`Library::function`]).
///
/// Repeated calls skip `dlsym` and `ffi_prep_cif` per call — the clone only
/// validates the arguments against the pinned signature, marshals them and
/// invokes. Bound to the scope that produced it, same as [`Library<'g>`].
///
/// Calling mirrors the fluent [`CallBuilder`] style:
/// `sqrt.call().arg(9.0).result::<f64>()?`.
#[derive(Debug)]
pub struct Function<'g>
{
  /// Clone-side handle returned by `FFIRequest::PrepareFunction`.
  handle: usize,

  /// Function name, for diagnostics in errors.
  name: String,

  /// Signature pinned at prepare time; mirrored parent-side for a local
  /// arity check that fails fast without an IPC round trip.
  argTypes: Vec<Type>,

  /// Lifetime marker tying the handle to the scope it was prepared in.
  _scope: PhantomData<&'g ()>
}

impl<'g> Function<'g>
{
  /// Starts a fluent prepared call. Arguments are checked against the
  /// pinned signature on both sides (here — arity only; the clone —
  /// full argument types).
  pub const fn call(&self) -> PreparedCallBuilder<'_, 'g>
  {
    PreparedCallBuilder { f: self, args: Vec::new() }
  }

  /// Test-only access to the clone-side handle (asserts distinctness).
  #[cfg(test)]
  pub(crate) fn handleForTests(&self) -> usize
  {
    self.handle
  }
}

/// Builder for fluent calls of a [`Function`] — the prepared analogue of
/// [`CallBuilder`]. The per-call work clone-side is validation + marshaling
/// + the call itself: no `dlsym`, no `ffi_prep_cif`.
pub struct PreparedCallBuilder<'a, 'g>
{
  /// The prepared function being called.
  f: &'a Function<'g>,

  /// Arguments collected for the call, in order.
  args: Vec<Value>
}

impl<'a, 'g> PreparedCallBuilder<'a, 'g>
{
  /// Append one argument. Chainable.
  #[inline]
  pub fn arg<T: FfiArg>(mut self, arg: T) -> Self
  {
    self.args.push(arg.intoFfiValue().0);
    self
  }

  /// Finalize: execute the prepared call and return a typed result.
  #[inline]
  pub fn result<T: FfiPrimitive>(self) -> Result<T, FFIError>
  {
    let raw: Value = self.finish()?;
    T::fromFfiValue(Arg(raw))
  }

  /// Finalize: execute the prepared call and return a struct by value.
  ///
  /// No field layout argument here (unlike [`CallBuilder::resultStruct`]):
  /// the return type was pinned at prepare time and drives the clone-side
  /// decoding, so there is nothing left to supply per call.
  #[inline]
  pub fn resultStruct(self) -> Result<StructValue, FFIError>
  {
    let raw: Value = self.finish()?;
    match raw {
      Value::Struct(values) => Ok(StructValue::fromValues(values)),
      other => Err(FFIError::Other(format!(
        "expected struct return, got {:?}", other
      )))
    }
  }

  /// Finalize: execute the prepared call and discard the result (void).
  #[inline]
  pub fn void(self) -> Result<(), FFIError>
  {
    self.finish()?;
    Ok(())
  }

  /// Sends the `CallPrepared` request after a local arity pre-check.
  fn finish(self) -> Result<Value, FFIError>
  {
    if ZygoteState.get().is_none() {
      return Err(FFIError::ZygoteNotInitialized);
    }

    // Local arity pre-check: fail fast without an IPC round trip. The clone
    // re-checks arity and full argument types anyway.
    if self.args.len() != self.f.argTypes.len() {
      return Err(FFIError::BadArgument(format!(
        "function {}: signature expects {} arguments, got {}",
        self.f.name, self.f.argTypes.len(), self.args.len()
      )));
    }

    sendRawRequest(FFIRequest::CallPrepared {
      handle: self.f.handle,
      args: self.args,
      readErrno: resolveReadErrno(None)
    })
  }
}

// =================================================================================================

#[cfg(test)]
mod tests
{
  use crate::ffi;
  use crate::ffi::allocatedMemory::AllocatedMemory;
  use crate::ffi::library::{getRegistry, Function};
  use crate::ffi::scope::Scope;
  use crate::ffi::types::Type;
  use crate::platform::{platformExt, LibcPath, LibmPath, OpenSymbolName, SprintfLibPath, SprintfSymbolName};
  // ===============================================================================================

  /// Checks that `.errno()` makes a failed call's errno observable via
  /// `Scope::lastErrno()` — `open()` on a path that can't exist sets ENOENT.
  #[test]
  fn errnoCapturedWhenRequested() -> ()
  {
    let errno: Option<i32> = ffi!(|scope| {
      let libc: Library = scope.load(LibcPath)?;
      let fd: i32 =
        libc.call(OpenSymbolName)
          .arg(c"/no/such/chillffi/test/path")
          .arg::<i32>(0 /* O_RDONLY */)
          .errno()
          .result()?;
      assert_eq!(fd, -1, "open() on a nonexistent path should fail");
      Ok(Scope::lastErrno())
    }).expect("errno capture test failed");

    assert_eq!(errno, Some(libc::ENOENT));
  }

  /// Checks that without `.errno()` (and no scope/global override), errno is
  /// not captured — `Scope::lastErrno()` stays `None` even after a call that
  /// itself set errno.
  #[test]
  fn errnoNoneWhenNotRequested() -> ()
  {
    let errno: Option<i32> = ffi!(|scope| {
      let libc: Library = scope.load(LibcPath)?;
      let fd: i32 =
        libc.call(OpenSymbolName)
          .arg(c"/no/such/chillffi/test/path2")
          .arg::<i32>(0)
          .result()?; // no .errno()
      assert_eq!(fd, -1);
      Ok(Scope::lastErrno())
    }).expect("errno-off test failed");

    assert_eq!(errno, None);
  }

  // ===============================================================================================
  
  /// Checks that `.errno()` captures the OS error via `Scope::lastOsError()`.
  #[cfg(windows)]
  #[test]
  fn osErrorCapturedWhenRequested() -> ()
  {
    const ErrorAccessDenied: u32 = 5;

    let osError: Option<u32> = ffi!(|scope| {
      scope.addSearchPath("examples/errno");
      let lib: Library = scope.load(platformExt!("liberrno"))?;
      let result: i32 =
        lib.call("failWithOsError")
          .arg::<u32>(ErrorAccessDenied)
          .errno()
          .result()?;
      assert_eq!(result, -1);
      Ok(Scope::lastOsError())
    }).expect("osError capture test failed");

    assert_eq!(osError, Some(ErrorAccessDenied));
  }

  /// Checks that without `.errno()`, `Scope::lastOsError()` stays `None`.
  #[cfg(windows)]
  #[test]
  fn osErrorNoneWhenNotRequested() -> ()
  {
    let osError: Option<u32> = ffi!(|scope| {
      scope.addSearchPath("examples/errno");
      let lib: Library = scope.load(platformExt!("liberrno"))?;
      let result: i32 =
        lib.call("failWithOsError")
          .arg::<u32>(5)
          .result()?; // no .errno()
      assert_eq!(result, -1);
      Ok(Scope::lastOsError())
    }).expect("osError-off test failed");

    assert_eq!(osError, None);
  }

  // ===============================================================================================
  
  /// Checks that library is removed from registry when explicitly dropped.
  #[test]
  fn libraryDrop() -> ()
  {
    let id: usize = ffi!(|scope| {
      let libm: Library = scope.load(LibmPath)?;
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
      let libm: Library = scope.load(LibmPath)?;
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
      let libm: Library = scope.load(LibmPath)?;
      let id: usize = libm.id();
      libm.unload()?;
      Ok(id)
    }).expect("ffi block failed");

    assert!(!getRegistry().read().contains_key(&id));
  }

  /// Checks that [`Library::path`] reports the resolved path a library was
  /// loaded through — with no scope/global search path registered, a bare
  /// name resolves to itself.
  #[test]
  fn path() -> ()
  {
    let path: String = ffi!(|scope| {
      let libm: Library = scope.load(LibmPath)?;
      Ok(libm.path().to_string())
    }).expect("ffi block failed");

    assert_eq!(path, LibmPath);
  }

  // ===============================================================================================

  /// `scope.load` never touches the filesystem — resolution is lazy, and
  /// `dlopen` only happens inside the clone on the first real call. Checks
  /// that a bad path is reported there, as [`FFIError::LibraryLoadFailed`].
  #[test]
  fn libraryLoadFailed() -> ()
  {
    use crate::ffi::errors::FFIError;

    let err: FFIError = ffi!(|scope| {
      let bogus: Library = scope.load(platformExt!("libChillffiDoesNotExist9000"))?;
      bogus.call("whatever").void()
    }).expect_err("loading a nonexistent library should fail");

    assert!(matches!(err, FFIError::LibraryLoadFailed{ .. }), "unexpected error: {err:?}");
  }

  /// The library itself loads fine — it's the symbol lookup inside it that
  /// fails, reported as [`FFIError::SymbolNotFound`].
  #[test]
  fn symbolNotFound() -> ()
  {
    use crate::ffi::errors::FFIError;

    let err: FFIError = ffi!(|scope| {
      let libm: Library = scope.load(LibmPath)?;
      libm.call("thisSymbolDoesNotExistAnywhere").void()
    }).expect_err("calling a missing symbol should fail");

    assert!(matches!(err, FFIError::SymbolNotFound{ .. }), "unexpected error: {err:?}");
  }

  // ===============================================================================================

  /// Checks calling the sqrt function from the libm library.
  #[test]
  fn sqrt() -> ()
  {
    let result: f64 = ffi!(|scope| {
      let libm: Library = scope.load(LibmPath)?;
      libm.call("sqrt").arg::<f64>(4.0).result()
    }).expect("FFI call failed");

    assert!((result - 2.0).abs() < f64::EPSILON);
  }

  /// Checks calling the abs function from the libm library.
  #[test]
  fn abs() -> ()
  {
    let result: i32 = ffi!(|scope| {
      let libm: Library = scope.load(LibmPath)?;
      libm.call("abs").arg::<i32>(-5).result()
    }).expect("FFI call failed");

    assert_eq!(result, 5);
  }

  // ===============================================================================================

  /// Prepared function: one `dlsym` + one `ffi_prep_cif` inside the clone,
  /// then repeated calls reuse them. Checks both correctness across repeated
  /// calls and that different prepared functions get different handles.
  #[test]
  fn preparedFunctionRoundtrip() -> ()
  {
    let (sum, handleA, handleB): (f64, usize, usize) = ffi!(|scope| {
      let libm: Library = scope.load(LibmPath)?;

      let sqrt: Function = libm.function("sqrt", &[Type::F64], Type::F64)?;
      let a: f64 = sqrt.call().arg(9.0).result()?;
      let b: f64 = sqrt.call().arg(16.0).result()?;
      let c: f64 = sqrt.call().arg(25.0).result()?;

      let fabs: Function = libm.function("fabs", &[Type::F64], Type::F64)?;

      Ok((a + b + c, sqrt.handleForTests(), fabs.handleForTests()))
    }).expect("prepared calls failed");

    assert!((sum - 12.0).abs() < 1e-9, "sqrt(9)+sqrt(16)+sqrt(25) != 12: {sum}");
    assert_ne!(handleA, handleB, "distinct prepared functions must not share a handle");
  }

  /// A signature mismatch must be a clean `BadArgument`, never UB:
  /// the pinned CIF is fixed, so an i64 into an f64 signature is rejected.
  #[test]
  fn preparedFunctionSignatureMismatch() -> ()
  {
    use crate::ffi::errors::FFIError;

    let err: FFIError = ffi!(|scope| {
      let libm: Library = scope.load(LibmPath)?;
      let sqrt: Function = libm.function("sqrt", &[Type::F64], Type::F64)?;
      sqrt.call().arg(9i64).result::<f64>()
    }).expect_err("i64 into an f64 signature must fail");

    assert!(matches!(err, FFIError::BadArgument(_)), "unexpected error: {err:?}");
  }

  /// Arity mismatch is caught parent-side, without an IPC round trip.
  #[test]
  fn preparedFunctionArityMismatch() -> ()
  {
    use crate::ffi::errors::FFIError;

    let err: FFIError = ffi!(|scope| {
      let libm: Library = scope.load(LibmPath)?;
      let sqrt: Function = libm.function("sqrt", &[Type::F64], Type::F64)?;
      sqrt.call().arg(9.0).arg(16.0).result::<f64>()
    }).expect_err("two args into a one-arg signature must fail");

    assert!(matches!(err, FFIError::BadArgument(_)), "unexpected error: {err:?}");
  }

  /// A missing symbol is reported at prepare time, not at call time.
  #[test]
  fn preparedFunctionSymbolNotFound() -> ()
  {
    use crate::ffi::errors::FFIError;

    // The error is extracted inside the block: `Function<'g>` cannot cross
    // the scope boundary — that is the lifetime gate doing its job.
    let err: FFIError = ffi!(|scope| {
      let libm: Library = scope.load(LibmPath)?;
      Ok(libm.function("thisSymbolDoesNotExistAnywhere", &[Type::F64], Type::F64)
        .err()
        .expect("preparing a missing symbol should fail"))
    }).expect("scope failed");

    assert!(matches!(err, FFIError::SymbolNotFound{ .. }), "unexpected error: {err:?}");
  }

  /// A prepared function cannot outlive its scope: `Function<'g>` borrows
  /// the scope's lifetime, so this must not compile. Kept as a comment —
  /// the borrow checker is the test:
  /// ```ignore
  /// fn steal<'a>(f: chillffi::ffi::library::Function<'a>) -> chillffi::ffi::library::Function<'a> { f }
  /// ```
  #[allow(unused)]
  fn preparedFunctionLifetimeIsTheGate() -> () {}

  // ===============================================================================================

  /// End-to-end variadic call: `sprintf(char *str, const char *format, ...)`
  /// — two fixed arguments followed by variadic ones, with the formatted
  /// string read back out of zygote memory.
  #[test]
  fn variadicSprintf() -> ()
  {
    let text: String = ffi!(|scope| {
      let libc: Library = scope.load(SprintfLibPath)?;
      let mem: AllocatedMemory = scope.alloc(64)?;

      let written: i32 = libc.call(SprintfSymbolName)
        .arg(mem.asPointer()) // char *str       — fixed 1
        .arg(c"Hello %s %d!") // const char *fmt — fixed 2
        .variadic()           // <- everything after this is variadic
        .arg(c"world")        // %s
        .arg::<i32>(42)       // %d
        .result()?;

      assert_eq!(written, 15, "sprintf should report 15 written chars");

      // mem.read() returns the whole allocation (64 bytes), including
      // uninitialized junk after the trailing NUL. Use sprintf's return
      // value as the exact length of the formatted C string.
      let bytes: Vec<u8> = mem.read()?;
      let text: String = String::from_utf8(bytes[..written as usize].to_vec())
        .expect("sprintf output must be valid UTF-8");

      Ok(text)
    }).expect("variadic sprintf failed");

    assert_eq!(text, "Hello world 42!");
  }

  /// `.variadic()` before any fixed argument is rejected: libffi's
  /// `ffi_prep_cif_var` requires at least one fixed argument
  /// (`nfixedargs >= 1`).
  #[test]
  fn variadicWithoutFixedArgsFails() -> ()
  {
    use crate::ffi::errors::FFIError;

    let err: FFIError = ffi!(|scope| {
      let libc: Library = scope.load(SprintfLibPath)?;
      libc.call(SprintfSymbolName)
        .variadic() // no fixed arguments before this
        .arg(c"hello")
        .result::<i32>()
    }).expect_err("a variadic call without fixed arguments should fail");

    assert!(matches!(err, FFIError::BadArgument(_)), "unexpected error: {err:?}");
  }

  // ===============================================================================================

  /// Checks repeated calls inside a single [`ffi!`] - uses cached dlopen.
  #[test]
  fn multipleCallsInSingleLibrary() -> ()
  {
    let results: Vec<f64> = ffi!(|scope| {
      let mut outputs: Vec<f64> = Vec::with_capacity(10);
      let libm: Library = scope.load(LibmPath)?;
  
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