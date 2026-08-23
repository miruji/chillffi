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
    Self { inner: UnsafeCell::new(None) }
  }
}

thread_local! {
  static ScopeStack: RefCell<Vec<*const ScopeGuard>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn resolveViaScope(name: &str) -> Option<String>
{
  ScopeStack.with(|s| {
    let ptr: *const ScopeGuard = *s.borrow().last()?;
    let slot: &Option<HeavyStack> = unsafe { &*(*ptr).inner.get() };
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
    unsafe {
      let slot: &mut Option<HeavyStack> = &mut *self.guard.inner.get();
      slot.get_or_insert_with(|| HeavyStack{ pathResolver: None })
        .pathResolver.get_or_insert_with(PathResolver::default)
        .addPath(path);
    }
  }

  // ===============================================================================================

  /// Allocates `length` bytes in the clone's heap.
  pub fn alloc(&self, length: usize) -> Result<AllocatedMemory<'g>, FFIError>
  {
    unsafe {
      let stack: &mut Option<HeavyStack> = &mut *self.guard.inner.get();

      // Initialization of the heavy stack happens only on the first call to alloc()
      if stack.is_none() {
        *stack = Some(HeavyStack{
          pathResolver: None
        });
      }
    }

    // Memory allocation through zigot
    match sendRawRequest(FFIRequest::Alloc { length })? {
      Value::Pointer(address) => Ok(AllocatedMemory::new(address, length)),
      _ => Err(FFIError::Other("Alloc did not return a pointer".to_string())),
    }
  }

  /// Frees memory previously obtained via `alloc` (or a C-side allocator).
  #[inline]
  pub fn free(pointer: usize) -> Result<(), FFIError>
  {
    sendRawRequest(FFIRequest::Free { pointer })?;
    Ok(())
  }

  /// Reads `length` bytes at `pointer` from the clone's memory.
  #[inline]
  pub fn readMemory(pointer: usize, length: usize) -> Result<Value, FFIError>
  {
    sendRawRequest(FFIRequest::ReadMemory { pointer, length })
  }

  /// Writes data from `Value` into the clone's memory at `pointer`.
  #[inline]
  pub fn writeMemory(pointer: usize, value: Value) -> Result<(), FFIError>
  {
    sendRawRequest(FFIRequest::WriteMemory { pointer, value })?;
    Ok(())
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
  use crate::ffi::errors::FFIError;
  use crate::ffi::scope::Scope;
  use crate::ffi::value::Value;
  use crate::ffi::value::Type;

  // ===============================================================================================

  /// Checks explicit memory release via Scope::free.
  #[test]
  fn free() -> ()
  {
    ffi!{
      let libc: Library = Library::load("libc.so.6")?;
      let ptr: Value = libc.call("malloc", vec![Value::Usize(16)], Type::Pointer)?;
      let Value::Pointer(addr) = ptr else {
        return Err(FFIError::Other("expected pointer".into()))
      };

      Scope::free(addr)?;
      Ok(())
    }.expect("Scope::free failed");
  }

  /// Checks reading memory allocated by C via Scope::readMemory.
  #[test]
  fn readMemory() -> ()
  {
    let bytes: Vec<u8> = ffi!{
      let libc: Library = Library::load("libc.so.6")?;
      let ptr: Value = libc.call("malloc", vec![Value::Usize(8)], Type::Pointer)?;
      let Value::Pointer(addr) = ptr else {
        return Err(FFIError::Other("expected pointer".into()))
      };

      libc.call("memset", vec![Value::Pointer(addr), Value::I32(0xAB), Value::Usize(8)], Type::Pointer)?;

      let Value::RawString(readBytes) = Scope::readMemory(addr, 8)? else {
        return Err(FFIError::Other("expected bytes".into()))
      };

      Scope::free(addr)?;
      Ok(readBytes)
    }.expect("Scope::readMemory failed");

    assert_eq!(bytes, vec![0xABu8; 8]);
  }

  /// Checks writing memory via Scope::writeMemory and reading it back through C.
  #[test]
  fn writeMemory() -> ()
  {
    let len: Value = ffi!{
      let libc: Library = Library::load("libc.so.6")?;
      let ptr: Value = libc.call("malloc", vec![Value::Usize(32)], Type::Pointer)?;
      let Value::Pointer(addr) = ptr else {
        return Err(FFIError::Other("expected pointer".into()))
      };

      Scope::writeMemory(addr, Value::CString(b"hello".to_vec()))?;

      let result: Value = libc.call("strlen", vec![Value::Pointer(addr)], Type::Usize)?;

      Scope::free(addr)?;
      Ok(result)
    }.expect("Scope::writeMemory failed");

    assert!(matches!(len, Value::Usize(5)));
  }

  // ===============================================================================================
}

// =================================================================================================