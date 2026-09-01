use crate::ffi::callback::{Callable, Sendable};
use serde::Serialize;
use crate::ffi::value::{Type, Primitive};
use std::sync::atomic::Ordering;
use std::sync::atomic::AtomicU64;
use std::cell::RefCell;
use std::path::PathBuf;
use crate::pathResolver::{PathResolver, resolveGlobal};
use std::cell::UnsafeCell;
use crate::ffi::allocatedMemory::AllocatedMemory;
use crate::ffi::errors::FFIError;
use crate::ffi::library::{sendRawRequest, nextLibraryId, registerLibrary, Library};
use crate::ffi::value::Value;
use crate::zygote::{ClonedZygote, FFIRequest, ZygoteGuard};
// =================================================================================================

/// Heavy stack or arena for temporary allocations within an [`ffi!`] scope.
struct HeavyStack
{
  /// Local path resolver for the current scope.
  pathResolver: Option<PathResolver>
}

// =================================================================================================

/// Owner of HeavyStack. Created by the [`ffi!`] macro once per block (only if
/// the user requested Scope), lives and dies strictly with this block.
///
/// Is not published directly — access only through [`Scope<'g>.`].
#[doc(hidden)]
pub struct ScopeGuard
{
  /// Lazily initialized internal state of the scope.
  inner: UnsafeCell<Option<HeavyStack>>
}

impl ScopeGuard
{
  #[doc(hidden)]
  #[inline(always)]
  pub const fn new() -> Self
  {
    Self {
      inner: UnsafeCell::new(None)
    }
  }
}

thread_local!{
  /// Thread-local stack tracking active ScopeGuard pointers for the current thread.
  static ScopeStack: RefCell<Vec<*const ScopeGuard>> = const { RefCell::new(Vec::new()) };
}

// =================================================================================================

/// A handle to the ScopeGuard of the current [`ffi!`]-block — borrows it for 'g.
///
/// That is precisely why [`AllocatedMemory<'g>`] and [`Library<'g>`] cannot leave
/// the block: the ScopeGuard, which they borrow, is dropped at the boundary of the
/// block, and this is checked by the compiler.
pub struct Scope<'g>
{
  guard: &'g ScopeGuard,
}

impl<'g> Scope<'g>
{
  // ===============================================================================================

  #[doc(hidden)]
  #[inline(always)]
  pub fn new(guard: &'g ScopeGuard) -> Self
  {
    ScopeStack.with(|s| s.borrow_mut().push(guard as *const ScopeGuard));
    Self { guard }
  }

  // ===============================================================================================

  /// Adds a directory to the local search path of the scope.
  pub fn addSearchPath(&self, path: impl Into<PathBuf>) -> ()
  {
    let slot: &mut Option<HeavyStack> = unsafe{ &mut *self.guard.inner.get() };
    slot.get_or_insert_with(|| HeavyStack{ pathResolver: None })
      .pathResolver.get_or_insert_with(PathResolver::default)
      .addPath(path);
  }

  // ===============================================================================================

  /// Loads a dynamic library and binds the handle to this scope — the same
  /// model as [`Scope::alloc`] / [`AllocatedMemory<'g>`].
  /// 
  /// Resolution order: this scope's local search path, then the global search path, 
  /// then the raw path as given.
  pub fn load(&self, libraryPath: &str) -> Result<Library<'g>, FFIError>
  {
    let slot: &Option<HeavyStack> = unsafe{ &*self.guard.inner.get() };
    let resolved: String = slot.as_ref()
      .and_then(|s| s.pathResolver.as_ref())
      .and_then(|r| r.resolve(libraryPath))
      .or_else(|| resolveGlobal(libraryPath))
      .unwrap_or_else(|| libraryPath.to_string());

    let libraryId: usize = nextLibraryId();
    registerLibrary(libraryId, &resolved);
    Ok(Library::new(libraryId, resolved))
  }

  // ===============================================================================================

  /// Allocates `length` bytes in the clone's heap.
  pub fn alloc(&self, length: usize) -> Result<AllocatedMemory<'g>, FFIError>
  {
    let stack: &mut Option<HeavyStack> = unsafe{ &mut *self.guard.inner.get() };

    // Initialization of the heavy stack happens only on the first call to alloc()
    if stack.is_none() {
      *stack = Some(HeavyStack{
        pathResolver: None
      });
    }

    // Memory allocation through zigot
    match sendRawRequest(FFIRequest::Alloc { length })? {
      Value::Pointer(address) => Ok(AllocatedMemory::new(address, length)),
      _ => Err(FFIError::Other("Alloc did not return a pointer".to_string())),
    }
  }

  /// Frees memory previously obtained via `alloc` (or a C-side allocator).
  #[inline]
  pub fn free(pointer: impl Into<usize>) -> Result<(), FFIError>
  {
    sendRawRequest(FFIRequest::Free { pointer: pointer.into() })?;
    Ok(())
  }

  /// Reads `length` bytes at `pointer` from the clone's memory.
  #[inline]
  pub fn readMemory(pointer: impl Into<usize>, length: usize) -> Result<Value, FFIError>
  {
    sendRawRequest(FFIRequest::ReadMemory { pointer: pointer.into(), length })
  }

  /// Writes data from [`Value`] into the clone's memory at `pointer`.
  #[inline]
  pub fn writeMemory(pointer: impl Into<usize>, value: Value) -> Result<(), FFIError>
  {
    sendRawRequest(FFIRequest::WriteMemory { pointer: pointer.into(), value })?;
    Ok(())
  }

  // ===============================================================================================

  /// Reads a dynamically-typed C struct at `pointer`, shaped by `fields`.
  pub fn readDynamicStruct(pointer: impl Into<usize>, fields: &[Type]) -> Result<Vec<Value>, FFIError>
  {
    match sendRawRequest(FFIRequest::ReadDynamicStruct { pointer: pointer.into(), fields: fields.to_vec() })? {
      Value::Struct(values) => Ok(values),
      other => Err(FFIError::Other(format!("ReadDynamicStruct: expected Value::Struct, got {:?}", other))),
    }
  }

  /// Writes `values` into a dynamically-typed C struct at `pointer`.
  pub fn writeDynamicStruct(pointer: impl Into<usize>, fields: &[Type], values: &[Value]) -> Result<(), FFIError>
  {
    sendRawRequest(FFIRequest::WriteDynamicStruct {
      pointer: pointer.into(), fields: fields.to_vec(), values: values.to_vec()
    })?;
    Ok(())
  }

  // ===============================================================================================

  /// Calls a raw function pointer directly — no `dlopen`/`dlsym`, the address
  /// is already known. Typical source: a pointer *returned* by a previous
  /// call (C ABI functions returning function pointers exist — e.g. libc's
  /// `signal()` both takes and returns one), or read out of a dispatch table
  /// via `readMemory`.
  pub fn callPointer<T: Primitive>(&self, pointer: impl Into<usize>, args: Vec<Value>) -> Result<T, FFIError>
  {
    let raw: Value = sendRawRequest(FFIRequest::CallPointer { pointer: pointer.into(), args, resultType: T::TypeTag })?;
    T::fromValue(raw)
  }

  /// Fire-and-forget variant of `callPointer` — mirrors `Library::callv`.
  #[inline]
  pub fn callvPointer(&self, pointer: impl Into<usize>, args: Vec<Value>) -> Result<(), FFIError>
  {
    self.callPointer::<()>(pointer, args)
  }

  // ===============================================================================================

  /// Registers a closure built with [`callback!`] as an FFI-callable function
  /// (e.g. a `qsort` comparator). Capture is explicit at the macro call site,
  /// this method only ships the already-built closure to the clone:
  pub fn callback<T>(&self, argTypes: Vec<Type>, returnType: Type, f: Sendable<Vec<Value>, Value, T>) -> Value
  where
    T: Callable<Vec<Value>, Value> + Serialize,
  {
    static nextID: AtomicU64 = AtomicU64::new(1);
    let id: u64 = nextID.fetch_add(1, Ordering::SeqCst);

    let bytes: Vec<u8> = f.encode().expect("encode callback");

    sendRawRequest(FFIRequest::RegisterCallback {
      id,
      bytes,
      argTypes,
      returnType,
    }).expect("register callback failed");

    Value::Function(id)
  }

  // ===============================================================================================
}

impl<'g> Drop for Scope<'g>
{
  fn drop(&mut self) -> () { ScopeStack.with(|s| { s.borrow_mut().pop(); }); }
}

// =================================================================================================

/// RAII owner of an isolated FFI execution context.
///
/// Created by [`FFIScope::enter`]. Holds:
/// - a cloned zygote process and its IPC socket — any FFI operation sent
///   through this thread's stack lands in this clone,
/// - a [`ScopeGuard`] that anchors the `'g` lifetime of [`Scope`],
///   [`AllocatedMemory<'g>`] and [`Library<'g>`].
///
/// On drop: the zygote is killed, the guard is released, and any
/// [`AllocatedMemory<'g>`] / [`Library<'g>`] still alive is *not* freed
/// automatically (their own `Drop` runs only while `'g` is still valid — by
/// construction it has been, because we are now at the end of the borrow).
/// 
/// todo Требует рассмотрения, такого по идее не должно быть возможно делать:
///  If you need to keep an allocation past the scope, extract its raw address
///  via [`AllocatedMemory::address`] before the scope ends.
///
/// This is the non-macro entry point. It exists for use cases where the
/// boundaries of the FFI block are not known at compile time — a JIT, an
/// interpreter, or code generated from another language.
pub struct FFIScope
{
  /// RAII handle that keeps the cloned zygote on the thread-local stack
  /// for as long as we are alive.
  _zygote: ZygoteGuard,
  /// Backing storage for the `'g` lifetime borrowed by [`Scope`],
  /// [`AllocatedMemory<'g>`] and [`Library<'g>`].
  guard: ScopeGuard,
}

impl FFIScope
{
  /// Enters a new isolated FFI context: forks a fresh zygote clone and
  /// prepares a scope guard. Returns an error if the global zygote has
  /// not been initialized or if the fork / IPC handshake fails.
  pub fn enter() -> Result<Self, FFIError>
  {
    let zygote: ClonedZygote = ClonedZygote::getMeClone()
      .map_err(|e| FFIError::Other(format!("failed to acquire zygote clone: {}", e)))?;
    let _zygote: ZygoteGuard = ZygoteGuard::enter(zygote);
    let guard: ScopeGuard = ScopeGuard::new();
    Ok(Self { _zygote, guard })
  }

  /// Borrows a [`Scope`] handle tied to this `FFIScope`'s lifetime.
  ///
  /// All [`AllocatedMemory<'g>`] / [`Library<'g>`] values obtained through this
  /// handle are freed (via their own `Drop`) no later than when this `FFIScope`
  /// is dropped — the compiler enforces that statically through `'g`.
  pub fn scope(&self) -> Scope<'_>
  {
    Scope::new(&self.guard)
  }
}

// =================================================================================================

// todo
//  In general, this is not entirely correct, scope is not used here. But protection that it is
//  used only inside a scope should be present. Therefore, this should be fixed.

/// Calls a raw function pointer through a [`Scope`].
#[macro_export]
macro_rules! callPointer
{
  ($scope:expr, $pointer:expr $(, $args:expr)* $(,)?) => {
    $scope.callPointer($pointer, vec![$($args.into()),*])
  };
}

/// Fire-and-forget variant of [`callPointer!`] — mirrors [`callv!`].
#[macro_export]
macro_rules! callvPointer
{
  ($scope:expr, $pointer:expr $(, $args:expr)* $(,)?) => {
    $scope.callvPointer($pointer, vec![$($args.into()),*])
  };
}

// =================================================================================================

#[cfg(test)]
mod tests
{
  use crate::ffi;
  use crate::ffi::errors::FFIError;
  use crate::ffi::library::Library;
  use crate::ffi::scope::Scope;
  use crate::ffi::value::{Pointer, Value};
  use crate::ffi::scope::FFIScope;
  // ===============================================================================================

  /// Checks explicit memory release via [`Scope::free`].
  #[test]
  fn free() -> ()
  {
    ffi!(|scope| {
      let libc: Library = scope.load("libc.so.6")?;
      let ptr: Pointer = libc.call("malloc").arg::<usize>(16).result()?;

      Scope::free(ptr)?;
      Ok(())
    }).expect("Scope::free failed");
  }

  /// Checks reading memory allocated by C via [`Scope::readMemory`].
  #[test]
  fn readMemory() -> ()
  {
    let bytes: Vec<u8> = ffi!(|scope| {
      let libc: Library = scope.load("libc.so.6")?;
      let ptr: Pointer = libc.call("malloc").arg::<usize>(8).result()?;

      libc.call("memset")
        .arg(ptr)
        .arg::<i32>(0xAB)
        .arg::<usize>(8)
        .void()?;

      let Value::RawString(readBytes) = Scope::readMemory(ptr, 8)? else {
        return Err(FFIError::Other("expected bytes".into()))
      };

      Scope::free(ptr)?;
      Ok(readBytes)
    }).expect("Scope::readMemory failed");

    assert_eq!(bytes, vec![0xABu8; 8]);
  }

  /// Checks writing memory via [`Scope::writeMemory`] and reading it back through C.
  #[test]
  fn writeMemory() -> ()
  {
    let len: usize = ffi!(|scope| {
      let libc: Library = scope.load("libc.so.6")?;
      let ptr: Pointer = libc.call("malloc").arg::<usize>(32).result()?;

      Scope::writeMemory(ptr, Value::CString(b"hello".to_vec()))?;

      let result: usize = libc.call("strlen").arg(ptr).result()?;

      Scope::free(ptr)?;
      Ok(result)
    }).expect("Scope::writeMemory failed");

    assert!(matches!(len, 5));
  }

  // ===============================================================================================

  /// Verify that a single [`FFIScope`] can be reused across multiple FFI operations.
  ///
  /// All operations use the same zygote and scope. The test also verifies that
  /// [`AllocatedMemory`] and [`Library`] remains tied to the scope lifetime.
  #[test]
  fn with_scope_retention() -> ()
  {
    // Direct RAII form: hold the scope ourselves and run several ops through it.
    let (r1, r2): (f64, i32) = (|| -> Result<_, FFIError>
    {
      let ffiScope: FFIScope = FFIScope::enter()?;
      let scope: Scope<'_> = ffiScope.scope();

      // 1. load libm, call sqrt(9.0) — shares the same zygote.
      let libm: Library = scope.load("libm.so.6")?;
      let r1: f64 = libm.call("sqrt").arg::<f64>(9.0).result()?;

      // 2. load libc, call abs(-7) on the SAME zygote clone.
      let libc: Library = scope.load("libc.so.6")?;
      let _abs: i32 = libc.call("abs").arg::<i32>(-7).result()?;

      // 3. scope.alloc + writeMemory + strlen — AllocatedMemory<'g> is
      // bounded by `ffiScope` (via `scope`), proving the 'g lifetime is real.
      let mem = scope.alloc(32)?;
      Scope::writeMemory(mem.address(), Value::CString(b"retained".to_vec()))?;
      let r2: usize = libc.call("strlen").arg(mem.address()).result()?;

      // mem drops here, sends Free, fine.
      Ok((r1, r2 as i32))
    })().expect("FFIScope flow failed");

    //
    assert!((r1 - 3.0).abs() < f64::EPSILON, "sqrt(9) != 3, got {}", r1);
    assert_eq!(r2, 8, "strlen(\"retained\") != 8, got {}", r2);
  }

  // ===============================================================================================
}

// =================================================================================================