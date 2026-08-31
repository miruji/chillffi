use crate::ffi::callback::{Callable, Sendable};
use serde::Serialize;
use crate::ffi::value::{Type, Primitive};
use std::sync::atomic::Ordering;
use std::sync::atomic::AtomicU64;
use std::cell::RefCell;
use std::path::PathBuf;
use crate::pathResolver::PathResolver;
use std::cell::UnsafeCell;
use crate::ffi::allocatedMemory::AllocatedMemory;
use crate::ffi::errors::FFIError;
use crate::ffi::library::sendRawRequest;
use crate::ffi::value::Value;
use crate::zygote::FFIRequest;
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

/// Resolves library name using local [`PathResolver`] of the innermost active scope.
pub(super) fn resolveViaScope(name: &str) -> Option<String>
{
  ScopeStack.with(|s| {
    let ptr: *const ScopeGuard = *s.borrow().last()?;
    let slot: &Option<HeavyStack> = unsafe{ &*(*ptr).inner.get() };
    slot.as_ref()?.pathResolver.as_ref()?.resolve(name)
  })
}

// =================================================================================================

/// A handle to the ScopeGuard of the current [`ffi!`]-block — borrows it for 'g.
///
/// That is precisely why [`AllocatedMemory<'g>`] cannot leave the block: the ScopeGuard,
/// which it borrows, is dropped at the boundary of the block, and this is checked by the compiler.
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
  use crate::ffi::scope::Scope;
  use crate::ffi::value::{Pointer, Value};
  // ===============================================================================================

  /// Checks explicit memory release via [`Scope::free`].
  #[test]
  fn free() -> ()
  {
    ffi!{
      let libc: Library = Library::load("libc.so.6")?;
      let ptr: Pointer = libc.call("malloc").arg::<usize>(16).result()?;

      Scope::free(ptr)?;
      Ok(())
    }.expect("Scope::free failed");
  }

  /// Checks reading memory allocated by C via [`Scope::readMemory`].
  #[test]
  fn readMemory() -> ()
  {
    let bytes: Vec<u8> = ffi!{
      let libc: Library = Library::load("libc.so.6")?;
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
    }.expect("Scope::readMemory failed");

    assert_eq!(bytes, vec![0xABu8; 8]);
  }

  /// Checks writing memory via [`Scope::writeMemory`] and reading it back through C.
  #[test]
  fn writeMemory() -> ()
  {
    let len: usize = ffi!{
      let libc: Library = Library::load("libc.so.6")?;
      let ptr: Pointer = libc.call("malloc").arg::<usize>(32).result()?;

      Scope::writeMemory(ptr, Value::CString(b"hello".to_vec()))?;

      let result: usize = libc.call("strlen").arg(ptr).result()?;

      Scope::free(ptr)?;
      Ok(result)
    }.expect("Scope::writeMemory failed");

    assert!(matches!(len, 5));
  }

  // ===============================================================================================
}

// =================================================================================================