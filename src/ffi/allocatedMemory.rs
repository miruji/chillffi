use crate::ffi::errors::FFIError;
use crate::ffi::library::sendRawRequest;
use crate::ffi::types::primitive::{FfiArg, Pointer};
use crate::ffi::types::Value;
use crate::zygote::FFIRequest;
use bytemuck::{Pod, Zeroable};
use std::marker::PhantomData;
// =================================================================================================

/// AllocatedMemory itself is needed when allocating memory on the Rust side;
/// It is an RAII wrapper over memory allocated on the heap of the zygote
/// clone via [`Scope::alloc`](crate::ffi::scope::Scope::alloc); Automatically sends a `Free` request 
/// when going out of scope (`Drop`).
///
/// Important: `Library` has its own methods for working with memory -
/// they are also needed, but only when we are not the creators of the memory region.
///
/// To work with raw addresses allocated by the C side (for example, `strdup`),
/// use the `Library` methods directly.
///
/// `'g` is the lifetime of the ScopeGuard block of [`ffi!`] in which it was created.
/// Until it is `'static` — the value physically cannot be returned from [`ffi!`] outside.
pub struct AllocatedMemory<'g>
{
  /// Raw address of the allocated memory block in the zygote heap.
  address: usize,
  /// Size of the allocated memory block in bytes.
  length: usize,
  /// Phantom lifetime marker tying the allocation to the [`ffi!`] scope.
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
  pub const fn address(&self) -> usize
  {
    self.address
  }
  /// Returns the size of the allocated memory block in bytes.
  pub const fn length(&self) -> usize
  {
    self.length
  }

  /// Wraps the address into a [`Pointer`] for FFI calls.
  pub const fn asPointer(&self) -> Pointer
  {
    Pointer(self.address)
  }

  /// Reads the entire allocated memory block from the zygote as raw bytes.
  pub fn read(&self) -> Result<Vec<u8>, FFIError>
  {
    let val: Value = sendRawRequest(
      FFIRequest::ReadMemory {
        pointer: self.address,
        length: self.length
      }
    )?;
    val.try_into()
  }

  /// Writes a value into the allocated memory block in the zygote.
  pub fn write(&self, value: impl FfiArg) -> Result<(), FFIError>
  {
    sendRawRequest(
      FFIRequest::WriteMemory {
        pointer: self.address,
        value: value.intoFfiValue().0
      }
    )?;
    Ok(())
  }

  // =================================================================================================

  /// Reads the allocated memory as a statically-typed C struct `T`.
  ///
  /// `T` must be `#[repr(C)]` and implement [`Pod`] (plain old data) from `bytemuck`.
  /// The `Pod` bound is checked at compile time — if `T` has padding or non-POD fields,
  /// the code will simply refuse to compile instead of silently reading garbage.
  ///
  /// This eliminates all manual byte parsing: the struct's size and field layout are
  /// guaranteed correct by the type system rather than by manually calculated offsets.
  pub fn readStruct<T: Pod + Zeroable>(&self) -> Result<T, FFIError>
  {
    let expectedSize: usize = size_of::<T>();
    if self.length < expectedSize {
      return Err(FFIError::Other(format!(
        "readStruct: buffer is {} bytes, but T is {} bytes",
        self.length, expectedSize
      )));
    }

    // Read raw bytes from zygote memory
    let bytes: Vec<u8> = self.read()?;
    if bytes.len() < expectedSize {
      return Err(FFIError::Other(format!(
        "readStruct: received {} bytes, expected at least {}",
        bytes.len(), expectedSize
      )));
    }

    // pod_read_unaligned reads T by value from raw bytes.
    // Safety is enforced at compile time by T: Pod (which guarantees
    // no padding, no uninit, no Drop, no invalid bit patterns).
    // If T has padding, derive(Pod) would fail and this wouldn't compile.
    Ok(bytemuck::pod_read_unaligned::<T>(&bytes[..expectedSize]))
  }

  /// Writes a statically-typed C struct `T` into the allocated memory buffer.
  ///
  /// `T` must be `#[repr(C)]` and implement [`Pod`] from `bytemuck`.
  ///
  /// The entire struct is serialized to bytes and written to the zygote's memory.
  pub fn writeStruct<T: Pod + Zeroable>(&self, value: &T) -> Result<(), FFIError>
  {
    let expectedSize: usize = size_of::<T>();
    if self.length < expectedSize {
      return Err(FFIError::Other(format!(
        "writeStruct: buffer is {} bytes, but T is {} bytes",
        self.length, expectedSize
      )));
    }

    // Convert struct to bytes using bytemuck
    let bytes: &[u8] = bytemuck::bytes_of(value);

    sendRawRequest(FFIRequest::WriteMemory {
      pointer: self.address,
      value: Value::RawString(bytes.to_vec())
    })?;
    Ok(())
  }
}

impl<'g> Drop for AllocatedMemory<'g>
{
  /// Automatically frees the allocated memory in the zygote on scope exit.
  fn drop(&mut self) -> ()
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
  use crate::platform::{LibcPath, TimeLibPath, TimeSymbolName};
  use bytemuck::{Pod, Zeroable};
  // ===============================================================================================

  /// Reading memory via [`AllocatedMemory::read`].
  #[test]
  fn read() -> ()
  {
    let bytes: Vec<u8> = ffi!(|scope| {
      let mem: AllocatedMemory = scope.alloc(8)?;

      let libc: Library = scope.load(LibcPath)?;
      // void *memset(void *s, int c, size_t n) — fills 8 bytes with 0xAB
      libc.call("memset")
        .arg(mem.asPointer())
        .arg::<i32>(0xAB)
        .arg::<usize>(8)
        .void()?;
      
      mem.read()
    }).expect("alloc/readMemory/free roundtrip failed");

    assert_eq!(bytes, vec![0xABu8; 8]);
  }

  /// Writing memory via [`AllocatedMemory::write`].
  #[test]
  fn write() -> ()
  {
    let len: usize = ffi!(|scope| {
      let mem: AllocatedMemory = scope.alloc(32)?;

      mem.write(c"hello")?;

      let libc: Library = scope.load(LibcPath)?;
      let result: usize = libc.call("strlen").arg(mem.asPointer()).result()?;

      Ok(result)
    }).expect("AllocatedMemory::write failed");

    assert!(matches!(len, 5));
  }

  // ===============================================================================================

  /// A simple C-like struct for readStruct/writeStruct.
  #[repr(C)]
  #[derive(Copy, Clone, Pod, Zeroable, Debug, PartialEq)]
  struct TestStruct
  {
    a: i64,
    b: i64
  }

  /// readStruct and writeStruct roundtrip.
  #[test]
  fn readWriteStruct() -> ()
  {
    let (original, read): (TestStruct, TestStruct) = ffi!(|scope| {
      let mem: AllocatedMemory = scope.alloc(size_of::<TestStruct>())?;

      let original: TestStruct = TestStruct { a: 42, b: 0x123456789ABCDEF0i64 };

      mem.writeStruct(&original)?;

      let read: TestStruct = mem.readStruct::<TestStruct>()?;

      Ok((original, read))
    }).expect("writeStruct/readStruct roundtrip failed");

    assert_eq!(original, read, "readStruct should return what was written");
  }

  /// Only memset, without a subsequent read.
  #[test]
  fn memsetOnly() -> ()
  {
    ffi!(|scope| {
    let mem: AllocatedMemory = scope.alloc(16)?;
    let libc: Library = scope.load(LibcPath)?;
    libc.call("memset")
      .arg(mem.asPointer())
      .arg::<i32>(0xFF)
      .arg::<usize>(16)
      .void()?;
    Ok(())
  }).expect("memset-only failed");
  }

  /// readStruct from a memset-filled buffer.
  #[test]
  fn readStructFromMemset() -> ()
  {
    let result: TestStruct = ffi!(|scope| {
      let mem: AllocatedMemory = scope.alloc(size_of::<TestStruct>())?;

      // Use memset to fill with a known pattern first
      let libc: Library = scope.load(LibcPath)?;
      libc.call("memset")
        .arg(mem.asPointer())
        .arg::<i32>(0xFF)
        .arg::<usize>(size_of::<TestStruct>())
        .void()?;

      mem.readStruct::<TestStruct>()
    }).expect("readStruct from memset failed");

    assert_eq!(result.a, 0xFFFFFFFFFFFFFFFFu64 as i64, "i64 should be 0xFFFFFFFFFFFFFFFF");
    assert_eq!(result.b, 0xFFFFFFFFFFFFFFFFu64 as i64, "i64 should be 0xFFFFFFFFFFFFFFFF");
  }

  /// readStruct via an FFI call into the OS wall clock: `clock_gettime` on
  /// Unix, `GetSystemTimeAsFileTime` on Windows — different layout, same
  /// readStruct.
  #[test]
  fn readStructFromFFI() -> ()
  {
    #[cfg(unix)]
    #[repr(C)]
    #[derive(Copy, Clone, Pod, Zeroable, Debug)]
    struct Clock { secs: i64, nanos: i64 }

    #[cfg(windows)]
    #[repr(C)]
    #[derive(Copy, Clone, Pod, Zeroable, Debug)]
    struct Clock { ticks: u64 }

    let clock: Clock = ffi!(|scope| {
      let clockLib: Library = scope.load(TimeLibPath)?;
      let mem: AllocatedMemory = scope.alloc(size_of::<Clock>())?;

      #[cfg(unix)]
      clockLib.call(TimeSymbolName)
        .arg::<i32>(0) // CLOCK_REALTIME
        .arg(mem.asPointer())
        .void()?;

      #[cfg(windows)]
      clockLib.call(TimeSymbolName)
        .arg(mem.asPointer())
        .void()?;

      mem.readStruct::<Clock>()
    }).expect("readStruct from the system clock failed");

    #[cfg(unix)]
    {
      assert!(clock.secs > 0, "seconds should be positive, got {}", clock.secs);
      assert!(
        clock.nanos >= 0 && clock.nanos < 1_000_000_000,
        "nanos should be in [0, 1e9), got {}", clock.nanos
      );
    }

    #[cfg(windows)]
    assert!(
      clock.ticks > crate::platform::FileTimeUnixEpoch,
      "FILETIME should be past the Unix epoch, got {}", clock.ticks
    );
  }

  // ===============================================================================================

  /// [`AllocatedMemory::readStruct`] must refuse a buffer smaller than
  /// `size_of::<T>()`, not silently read past the allocation.
  #[test]
  fn readStructBufferTooSmall() -> ()
  {
    use crate::ffi::errors::FFIError;

    let err: FFIError = ffi!(|scope| {
      // TestStruct is 16 bytes; this buffer is only 4.
      let mem: AllocatedMemory = scope.alloc(4)?;
      Ok(mem.readStruct::<TestStruct>())
    }).expect("ffi block failed")
      .expect_err("readStruct into an undersized buffer should fail");

    assert!(matches!(err, FFIError::Other(_)), "unexpected error: {err:?}");
  }

  /// Write-side mirror: [`AllocatedMemory::writeStruct`] must refuse to
  /// write a `T` bigger than the buffer it was allocated with.
  #[test]
  fn writeStructBufferTooSmall() -> ()
  {
    use crate::ffi::errors::FFIError;

    let err: FFIError = ffi!(|scope| {
      let mem: AllocatedMemory = scope.alloc(4)?;
      let value: TestStruct = TestStruct { a: 1, b: 2 };
      Ok(mem.writeStruct(&value))
    }).expect("ffi block failed")
      .expect_err("writeStruct into an undersized buffer should fail");

    assert!(matches!(err, FFIError::Other(_)), "unexpected error: {err:?}");
  }

  // ===============================================================================================
}

// =================================================================================================