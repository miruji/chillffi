use std::marker::PhantomData;
use crate::ffi::errors::FFIError;
use crate::ffi::library::sendRawRequest;
use crate::ffi::value::Value;
use crate::zygote::FFIRequest;
// =================================================================================================

/// AllocatedMemory itself is needed when allocating memory on the Rust side;
/// It is an RAII wrapper over memory allocated on the heap of the zygote
/// clone via `Library::alloc`; Automatically sends a `Free` request 
/// when going out of scope (`Drop`).
///
/// Important: `Library` has its own methods for working with memory -
/// they are also needed, but only when we are not the creators of the memory region.
///
/// To work with raw addresses allocated by the C side (for example, `strdup`),
/// use the `Library` methods directly.
///
/// `'g` is the lifetime of the ScopeGuard block of `ffi!{}` in which it was created.
/// Until it is `'static` — the value physically cannot be returned from `ffi!{}` outside.
pub struct AllocatedMemory<'g>
{
  /// Raw address of the allocated memory block in the zygote heap.
  address: usize,
  /// Size of the allocated memory block in bytes.
  length: usize,
  /// Phantom lifetime marker tying the allocation to the ffi!{} scope.
  _scope: PhantomData<&'g ()>
}

impl<'g> AllocatedMemory<'g>
{
  /// Creates a new wrapper for a raw zygote allocation.
  pub(super) const fn new(address: usize, length: usize) -> Self 
  {
    Self {
      address,
      length,
      _scope: PhantomData,
    }
  }

  /// Returns the raw memory address of the allocation.
  pub const fn address(&self) -> usize { self.address }
  /// Returns the size of the allocated memory block in bytes.
  pub const fn length(&self) -> usize { self.length }

  /// Wraps the address into a Value::Pointer for FFI calls.
  pub const fn asPointer(&self) -> Value {
    Value::Pointer(self.address)
  }

  /// Reads the entire allocated memory block from the zygote.
  pub fn read(&self) -> Result<Value, FFIError> {
    sendRawRequest(FFIRequest::ReadMemory { pointer: self.address, length: self.length })
  }

  /// Writes a value into the allocated memory block in the zygote.
  pub fn write(&self, value: Value) -> Result<(), FFIError> {
    sendRawRequest(FFIRequest::WriteMemory { pointer: self.address, value })?;
    Ok(())
  }
}

impl<'g> Drop for AllocatedMemory<'g>
{
  /// Automatically frees the allocated memory in the zygote on scope exit.
  fn drop(&mut self) 
  {
    if self.address != 0 {
      let _ = sendRawRequest(FFIRequest::Free { pointer: self.address });
    }
  }
}


// =================================================================================================

#[cfg(test)]
mod tests
{
  use crate::ffi;
  use crate::call;
  use crate::callv;
  use crate::ffi::allocatedMemory::AllocatedMemory;
  use crate::ffi::errors::FFIError;
  use crate::ffi::value::Value;
  // ===============================================================================================

  /// Checks reading memory via AllocatedMemory::read.
  #[test]
  fn read() -> ()
  {
    let bytes: Vec<u8> = ffi!(|scope| {
      let mem: AllocatedMemory = scope.alloc(8)?;

      let libc: Library = Library::load("libc.so.6")?;
      // void *memset(void *s, int c, size_t n) — fills 8 bytes with 0xAB
      callv!(libc, "memset", mem.asPointer(), 0xAB as i32, 8 as usize)?;

      let Value::RawString(bytes) = mem.read()? else { 
        return Err(FFIError::Other("expected bytes".into())) 
      };

      Ok(bytes)
    }).expect("alloc/readMemory/free roundtrip failed");

    assert_eq!(bytes, vec![0xABu8; 8]);
  }
  
  /// Checks writing memory via AllocatedMemory::write.
  #[test]
  fn write() -> ()
  {
    let len: usize = ffi!(|scope| {
      let mem: AllocatedMemory = scope.alloc(32)?;

      mem.write(Value::CString(b"hello".to_vec()))?;

      let libc: Library = Library::load("libc.so.6")?;
      let result: usize = call!(libc, "strlen", mem.asPointer())?;

      Ok(result)
    }).expect("AllocatedMemory::write failed");

    assert!(matches!(len, 5));
  }

  /// Checks automatic deallocation via Drop when AllocatedMemory leaves scope.
  #[test]
  fn drop() -> ()
  {
    let (addr1, addr2): (usize, usize) = ffi!(|scope| {
      let addr1: usize = {
        let mem: AllocatedMemory = scope.alloc(16)?;
        let a: usize = mem.address();
        // mem is dropped here, sending Free
        a
      };

      let mem2: AllocatedMemory = scope.alloc(16)?;
      let addr2: usize = mem2.address();

      Ok((addr1, addr2))
    }).expect("AllocatedMemory::drop failed");

    // If Drop freed the first allocation, malloc may reuse the same address
    assert_eq!(addr1, addr2);
  }

  // ===============================================================================================
}

// =================================================================================================