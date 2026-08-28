use crate::ffi::callback::{Callable, Sendable};
use serde::Serialize;
use crate::ffi::value::Type;
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

/// Heavy stack or arena for temporary allocations within an ffi!{} scope.
struct HeavyStack
{
  /// Local path resolver for the current scope.
  pathResolver: Option<PathResolver>
}

// =================================================================================================

/// Owner of HeavyStack. Created by the ffi!{} macro once per block (only if
/// the user requested Scope), lives and dies strictly with this block.
///
/// Is not published directly — access only through Scope<'g>.
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

/// Resolves library name using local PathResolver of the innermost active scope.
pub(super) fn resolveViaScope(name: &str) -> Option<String>
{
  ScopeStack.with(|s| {
    let ptr: *const ScopeGuard = *s.borrow().last()?;
    let slot: &Option<HeavyStack> = unsafe{ &*(*ptr).inner.get() };
    slot.as_ref()?.pathResolver.as_ref()?.resolve(name)
  })
}

// =================================================================================================

/// A handle to the ScopeGuard of the current ffi!{}-block — borrows it for 'g.
///
/// That is precisely why AllocatedMemory<'g> cannot leave the block: the ScopeGuard,
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

  /// Writes data from `Value` into the clone's memory at `pointer`.
  #[inline]
  pub fn writeMemory(pointer: impl Into<usize>, value: Value) -> Result<(), FFIError>
  {
    sendRawRequest(FFIRequest::WriteMemory { pointer: pointer.into(), value })?;
    Ok(())
  }

  // ===============================================================================================

  /// Registers a closure built with `callback!` as an FFI-callable function
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

#[cfg(test)]
mod tests
{
  use crate::ffi;
  use crate::call;
  use crate::callv;
  use crate::ffi::errors::FFIError;
  use crate::ffi::scope::Scope;
  use crate::ffi::value::{Pointer, Value};
  // ===============================================================================================

  /// Checks explicit memory release via Scope::free.
  #[test]
  fn free() -> ()
  {
    ffi!{
      let libc: Library = Library::load("libc.so.6")?;
      let ptr: Pointer = call!(libc, "malloc", 16 as usize)?;

      Scope::free(ptr)?;
      Ok(())
    }.expect("Scope::free failed");
  }

  /// Checks reading memory allocated by C via Scope::readMemory.
  #[test]
  fn readMemory() -> ()
  {
    let bytes: Vec<u8> = ffi!{
      let libc: Library = Library::load("libc.so.6")?;
      let ptr: Pointer = call!(libc, "malloc", 8 as usize)?;

      callv!(libc, "memset", ptr, 0xAB as i32, 8 as usize)?;

      let Value::RawString(readBytes) = Scope::readMemory(ptr, 8)? else {
        return Err(FFIError::Other("expected bytes".into()))
      };

      Scope::free(ptr)?;
      Ok(readBytes)
    }.expect("Scope::readMemory failed");

    assert_eq!(bytes, vec![0xABu8; 8]);
  }

  /// Checks writing memory via Scope::writeMemory and reading it back through C.
  #[test]
  fn writeMemory() -> ()
  {
    let len: usize = ffi!{
      let libc: Library = Library::load("libc.so.6")?;
      let ptr: Pointer = call!(libc, "malloc", 32 as usize)?;

      Scope::writeMemory(ptr, Value::CString(b"hello".to_vec()))?;

      let result: usize = call!(libc, "strlen", ptr)?;

      Scope::free(ptr)?;
      Ok(result)
    }.expect("Scope::writeMemory failed");

    assert!(matches!(len, 5));
  }

  // ===============================================================================================
}

// =================================================================================================