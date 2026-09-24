use crate::__ffiInternal::ClonedZygote;
use crate::errnoPolicy::globalReadErrno;
use crate::ffi::errors::FFIError;
use crate::ffi::scope::currentScopeReadErrno;
use crate::ffi::scope::currentScopeTrace;
use crate::ffi::types::primitive::{Arg, FfiArg, FfiPrimitive, StructValue};
use crate::ffi::types::Type;
use crate::ffi::types::Value;
use crate::tracePolicy::envTrace;
use crate::tracePolicy::globalTrace;
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

/// Resolves the effective trace flag for a call — same most-specific-wins
/// order as [`resolveReadErrno`]:
/// 1. per-call override — `.trace()` / `.noTrace()` on the call builder
/// 2. scope-level override — [`Scope::setTrace`](crate::ffi::scope::Scope::setTrace)
/// 3. global default — [`crate::tracePolicy::setGlobalTrace`] / `ChillffiTrace`
///
/// The resolved value controls both the Runtime-side trace (this process logs
/// the request it's about to send and the response it gets back) and the
/// `trace` field placed into [`FFIRequest::Call`] / [`FFIRequest::CallPointer`]
/// so the clone side logs its own work on the same call.
#[inline]
pub(super) fn resolveTrace(perCall: Option<bool>) -> bool
{
  perCall
    .unwrap_or_else(|| currentScopeTrace()
      .unwrap_or_else(globalTrace))
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

  // Runtime-side trace: the request is about to be sent. The flag was
  // already resolved by the caller (`callById` / `Scope::callPointerImpl`)
  // and baked into `Call`/`CallPointer`'s `trace` field — for non-Call
  // variants the global default decides via `resolveTrace(None)`.
  //
  // `envTrace()` is checked first and OR-ed in: `ChillffiTrace=1` is the
  // "debug everything" master switch — when set, the Runtime side logs
  // every request regardless of `.noTrace()` overrides. Same shape as the
  // clone side's `cloneEnvTrace() || requestTraceFlag` check, so the two
  // sides stay symmetric.
  let traceOn: bool = envTrace()
    || match &request {
      FFIRequest::Call { trace, .. } | FFIRequest::CallPointer { trace, .. } => *trace,
      _ => resolveTrace(None)
    };
  if traceOn && !cfg!(test) {
    eprintln!("[chillffi] send {:?}", request);
  }

  // Retrieve the most recently pushed active zygote
  // from the thread-local stack to execute the raw FFI request.
  let responseResult: Result<FFIResponse, FFIError> = ZygoteStack.with(|stack| {
    let mut mutStack: RefMut<Vec<ClonedZygote>> = stack.borrow_mut();
    let zygote: &mut ClonedZygote = mutStack.last_mut().ok_or(FFIError::NoActiveZygoteScope)?;

    match zygote.call(request) {
      Ok(FFIResponse::Ok(val, errno, osError)) => {
        LastErrno.set(errno);
        LastOsError.set(osError);
        Ok(FFIResponse::Ok(val, errno, osError))
      }
      Ok(FFIResponse::Err(err)) => Err(err),
      Err(err) => Err(FFIError::ZygoteCommunicationFailed(err))
    }
  });

  // Runtime-side trace: pair with the `send` log above. Logging the
  // response (Ok value / Err variant) — or the IPC-level communication
  // failure if the clone died before replying — closes the loop on what
  // the parent process observed for this request.
  if traceOn && !cfg!(test) 
  {
    match &responseResult 
    {
      Ok(FFIResponse::Ok(val, errno, osError)) =>
        eprintln!("[chillffi] recv ok  {:?} errno={:?} osError={:?}", val, errno, osError),
      Ok(FFIResponse::Err(err)) =>
        eprintln!("[chillffi] recv err {:?}", err),
      Err(commErr) =>
        eprintln!("[chillffi] recv err (clone unreachable): {}", commErr)
    }
  }

  responseResult.map(|r| match r {
    FFIResponse::Ok(val, _, _) => val,
    // The Err arm already turned into `Err(commErr)` or `Err(err)` above;
    // pulling it out of the response here can't fail.
    FFIResponse::Err(_) => unreachable!("FFIResponse::Err handled above")
  })
}

/// Performs an FFI function call by the identifier of the registered library.
//
// `clippy::too_many_arguments`: 8 args is one over the default limit, but
// each is genuinely needed — `libraryId` + `libraryPath` together resolve
// the lib, `functionName`/`args`/`resultType` describe the call, and
// `readErrno` / `trace` / `fixedArgs` are three orthogonal flags that all
// ride on the same `FFIRequest::Call`. Splitting them into a struct just
// to satisfy the lint would hide what's actually being sent.
#[allow(clippy::too_many_arguments)]
fn callById(
  libraryId: usize,
  libraryPath: &str,
  functionName: &str,
  args: Vec<Value>,
  resultType: Type,
  readErrno: bool,
  trace: bool,
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
    trace,
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
  readErrno: Option<bool>,

  /// Per-call override of trace output. `None` falls through to the
  /// enclosing scope's setting, then the global default / `ChillffiTrace` —
  /// see [`resolveTrace`]. When `Some(true)`, this single call gets a
  /// full send/recv log on the Runtime side *and* the clone side (the
  /// `trace` flag rides on the request).
  trace: Option<bool>
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
      readErrno: None,
      trace: None
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

  /// Forces trace output for this call specifically, regardless of the
  /// scope's or global default / `ChillffiTrace`. Logs the request and
  /// response on the Runtime side *and* the clone side (the `trace` flag
  /// rides on the request so the forked worker logs its own work too).
  #[inline]
  pub const fn trace(mut self) -> Self
  {
    self.trace = Some(true);
    self
  }

  /// Forces trace output *off* for this call, overriding a scope/global
  /// default (or `ChillffiTrace=1`) that would otherwise have enabled it.
  #[inline]
  pub const fn noTrace(mut self) -> Self
  {
    self.trace = Some(false);
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
    let trace: bool = resolveTrace(self.trace);
    self.lib.__call(&self.name, self.args, readErrno, trace, None)
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
    let trace: bool = resolveTrace(self.trace);
    let resultType: Type = Type::structure(fields.iter().cloned());
    let raw: Value = callById(
      self.lib.id(),
      self.lib.path(),
      &self.name,
      self.args,
      resultType,
      readErrno,
      trace,
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
    let trace: bool = resolveTrace(self.trace);
    self.lib.__call::<()>(&self.name, self.args, readErrno, trace, None).map(|_| ())
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

  /// Forces trace output for this call specifically — see
  /// [`CallBuilder::trace`](crate::ffi::library::CallBuilder::trace).
  #[inline]
  pub const fn trace(mut self) -> Self
  {
    self.base.trace = Some(true);
    self
  }

  /// Forces trace output *off* for this call — see
  /// [`CallBuilder::noTrace`](crate::ffi::library::CallBuilder::noTrace).
  #[inline]
  pub const fn noTrace(mut self) -> Self
  {
    self.base.trace = Some(false);
    self
  }

  /// Finalize: execute the variadic call and return a typed result.
  #[inline]
  pub fn result<T: FfiPrimitive>(self) -> Result<T, FFIError>
  {
    let readErrno: bool = resolveReadErrno(self.base.readErrno);
    let trace: bool = resolveTrace(self.base.trace);
    self.base.lib.__call(&self.base.name, self.base.args, readErrno, trace, Some(self.fixedArgsCount))
  }

  /// Finalize: execute the variadic call and return a struct by value.
  ///
  /// Same contract as [`CallBuilder::resultStruct`] — the field layout must
  /// be supplied explicitly so the return buffer can be decoded.
  #[inline]
  pub fn resultStruct(self, fields: &[Type]) -> Result<StructValue, FFIError>
  {
    let readErrno: bool = resolveReadErrno(self.base.readErrno);
    let trace: bool = resolveTrace(self.base.trace);
    let resultType: Type = Type::structure(fields.iter().cloned());
    let raw: Value = callById(
      self.base.lib.id(),
      self.base.lib.path(),
      &self.base.name,
      self.base.args,
      resultType,
      readErrno,
      trace,
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
    let trace: bool = resolveTrace(self.base.trace);
    self.base.lib.__call::<()>(&self.base.name, self.base.args, readErrno, trace, Some(self.fixedArgsCount))
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
    trace: bool,
    fixedArgs: Option<usize>
  ) -> Result<T, FFIError>
  {
    let raw: Value = callById(
      self.libraryId,
      &self.libraryPath,
      functionName, args,
      T::TypeTag,
      readErrno,
      trace,
      fixedArgs
    )?;
    T::fromFfiValue(Arg(raw))
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
  use crate::ffi::allocatedMemory::AllocatedMemory;
  use crate::ffi::library::getRegistry;
  use crate::ffi::scope::Scope;
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
  //  Trace
  // ===============================================================================================

  /// Trace is a side-effect-only addition — turning it on must not change
  /// the call's observable result. `sqrt(16.0)` with `.trace()` returns 4.0
  /// exactly as it would without it.
  #[test]
  fn traceCallDoesNotBreakResult() -> ()
  {
    let result: f64 = ffi!(|scope| {
      let libm: Library = scope.load(LibmPath)?;
      libm.call("sqrt").arg::<f64>(16.0).trace().result()
    }).expect("traced sqrt failed");

    assert!((result - 4.0).abs() < f64::EPSILON, "trace must not alter the call result");
  }

  /// `.noTrace()` overrides `Scope::setTrace(true)` — per-call override wins
  /// over scope default, same priority rule as `.noErrno()` / `setReadErrno`.
  /// Result is unchanged either way; this test pins the priority rule.
  #[test]
  fn noTraceOverridesScopeTrace() -> ()
  {
    let result: f64 = ffi!(|scope| {
      scope.setTrace(true);
      let libm: Library = scope.load(LibmPath)?;
      libm.call("sqrt").arg::<f64>(25.0).noTrace().result()
    }).expect("noTrace+scope-trace call failed");

    assert!((result - 5.0).abs() < f64::EPSILON, "result must be unchanged by trace state");
  }

  /// `Scope::setTrace(true)` traces every call in the block — same
  /// side-effect-only contract: result is unchanged by trace state.
  #[test]
  fn scopeTraceDoesNotBreakResult() -> ()
  {
    let result: f64 = ffi!(|scope| {
      scope.setTrace(true);
      let libm: Library = scope.load(LibmPath)?;
      libm.call("sqrt").arg::<f64>(36.0).result()
    }).expect("scope-trace sqrt failed");

    assert!((result - 6.0).abs() < f64::EPSILON);
  }

  /// Variadic call path: `.trace()` on a `VariadicCallBuilder` chain. Same
  /// guarantee — the call still produces the right value with trace on.
  /// Uses `sprintf` because it exercises both the variadic ABI and a pointer
  /// out-param, exactly the kind of call where trace is most useful.
  #[test]
  fn variadicTraceDoesNotBreakResult() -> ()
  {
    use crate::ffi::allocatedMemory::AllocatedMemory;

    let text: String = ffi!(|scope| {
      let libc: Library = scope.load(SprintfLibPath)?;
      let mem: AllocatedMemory = scope.alloc(64)?;

      let written: i32 = libc.call(SprintfSymbolName)
        .arg(mem.asPointer())
        .arg(c"%d-trace")
        .variadic()
        .arg::<i32>(7)
        .trace()
        .result()?;

      let bytes: Vec<u8> = mem.read()?;
      let text: String = String::from_utf8(bytes[..written as usize].to_vec())
        .expect("sprintf output must be valid UTF-8");
      Ok(text)
    }).expect("variadic+trace sprintf failed");

    assert_eq!(text, "7-trace", "trace must not alter the variadic call result");
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