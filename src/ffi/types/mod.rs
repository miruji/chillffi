// =================================================================================================
pub mod primitive;
// =================================================================================================
use serde::{Deserialize, Serialize};
// =================================================================================================

/// A value that can be passed between processes and used when calling FFI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) enum Value
{
  /// Just an empty value.
  None,

  //
  U8(u8),
  U16(u16),
  U32(u32),
  U64(u64),
  Usize(usize),

  //
  I8(i8),
  I16(i16),
  I32(i32),
  I64(i64),
  Isize(isize),

  //
  F32(f32),
  F64(f64),

  //
  Bool(bool),

  /// Store the address as a regular number;
  ///
  /// The address is stored as usize rather than as a raw pointer — for two reasons:
  /// serialization through bincode/serde (raw pointers cannot do this)
  /// and the fact that the owner of the memory is the C code on the zygote clone side,
  /// not Rust.
  ///
  /// # Lifetime
  /// Valid strictly within the same ffi!{} block — that is, inside the same
  /// zygote clone. The clone dies when leaving the block
  /// (ZygoteGuard::drop → SIGKILL), and along with it dies the address
  /// space to which this address belonged.
  ///
  /// Using it outside the block is undefined behavior, not an Err:
  /// a new clone is forked from the same parent zygote and often has
  /// the same mapped address space, therefore at that address
  /// there may be unrelated memory instead of the expected crash.
  ///
  /// Ownership and deallocation (free/strdup and similar) are exclusively on
  /// the side of the calling C code.
  Pointer(usize),

  /// In C code, one would expect `uint8_t *data`;
  /// But without `len` these bytes are useless and `size_t len` is necessary.
  RawString(Vec<u8>),

  /// In C code, one would expect `const char *str`; `\0` terminated.
  CString(Vec<u8>),

  /// In C code, one would expect `const char *str, size_t len`.
  String(Vec<u8>),

  /// Represents a Rust closure passed to C as a function pointer.
  ///
  /// The `u64` holds the unique ID used to locate the JIT-compiled trampoline 
  /// inside the clone's callback registry.
  Function(u64),

  /// An ordered list of fields (not named).
  Struct(Vec<Self>)
}

// =================================================================================================

/// Description of the value type 
/// for defining FFI arguments and result.
///
/// todo: Add unit tests for Type variants verification.
///
/// todo: In general, it should be completely hidden and unavailable for separate use.
#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
#[doc(hidden)]
pub enum Type
{
  /// Just an empty value.
  None,

  //
  U8,
  U16,
  U32,
  U64,
  Usize,

  //
  I8,
  I16,
  I32,
  I64,
  Isize,

  //
  F32,
  F64,

  //
  Bool,

  /// Raw pointer.
  Pointer,

  /// An ordered list of fields (not named).
  Struct(Vec<Self>)
}

// =================================================================================================