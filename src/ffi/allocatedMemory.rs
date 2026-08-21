use std::marker::PhantomData;
use crate::ffi::errors::FFIError;
use crate::ffi::library::sendRawRequest;
use crate::ffi::value::Value;
use crate::zygote::FFIRequest;
// =================================================================================================

/// AllocatedMemory сама по себе нужна при выделении памяти со стороны Rust;
/// Это RAII-обёртка над памятью, выделенной в куче клона зиготы через `Library::alloc`;
/// Автоматически отправляет запрос `Free` при выходе из области видимости (`Drop`).
/// 
/// Важно: В `Library` есть свои методы для работы с памятью - 
/// они тоже нужны, но уже когда мы, не являемся создателями участка памяти.
/// 
/// Для работы с сырыми адресами, выделенными C-стороной (например, `strdup`),
/// используйте напрямую методы `Library` напрямую.
/// 
/// 'g — время жизни ScopeGuard блока ffi!{}, в котором она создана.
/// Пока это не 'static — значение физически нельзя вернуть из ffi!{} наружу.
pub struct AllocatedMemory<'g>
{
  /// todo desc
  address: usize,
  /// todo desc
  length: usize,
  /// todo desc
  _scope: PhantomData<&'g ()>
}

impl<'g> AllocatedMemory<'g>
{
  /// todo desc
  pub(super) fn new(address: usize, length: usize) -> Self 
  {
    Self {
      address,
      length,
      _scope: PhantomData,
    }
  }

  /// todo desc
  pub fn address(&self) -> usize { self.address }
  /// todo desc
  pub fn length(&self) -> usize { self.length }

  /// todo desc
  pub fn asPointer(&self) -> Value {
    Value::Pointer(self.address)
  }

  /// todo desc
  pub fn read(&self) -> Result<Value, FFIError> {
    sendRawRequest(FFIRequest::ReadMemory { pointer: self.address, length: self.length })
  }

  /// todo desc
  pub fn write(&self, value: Value) -> Result<(), FFIError> {
    sendRawRequest(FFIRequest::WriteMemory { pointer: self.address, value })?;
    Ok(())
  }
}

impl<'g> Drop for AllocatedMemory<'g>
{
  /// todo desc
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
  use crate::ffi::allocatedMemory::AllocatedMemory;
  use crate::ffi::errors::FFIError;
  use crate::ffi::value::Value;
  use crate::ffi::value::Type;
  // ===============================================================================================

  /// Checks reading memory via AllocatedMemory::read.
  #[test]
  fn read() -> ()
  {
    let bytes: Vec<u8> = ffi!(|scope| {
      let mem: AllocatedMemory = scope.alloc(8)?;

      let libc: Library = Library::load("libc.so.6")?;
      // void *memset(void *s, int c, size_t n) — fills 8 bytes with 0xAB
      libc.call("memset", vec![mem.asPointer(), Value::I32(0xAB), Value::Usize(8)], Type::Pointer)?;

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
    let len: Value = ffi!(|scope| {
      let mem: AllocatedMemory = scope.alloc(32)?;

      mem.write(Value::CString(b"hello".to_vec()))?;

      let libc: Library = Library::load("libc.so.6")?;
      let result: Value = libc.call("strlen", vec![mem.asPointer()], Type::Usize)?;

      Ok(result)
    }).expect("AllocatedMemory::write failed");

    assert!(matches!(len, Value::Usize(5)));
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