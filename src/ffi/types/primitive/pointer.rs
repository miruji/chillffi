use crate::ffi::errors::FFIError;
use crate::ffi::types::primitive::Primitive;
use crate::ffi::types::{Value, Type};
// =================================================================================================

/// Wrapper for a raw memory address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pointer(pub usize);

// =================================================================================================

impl Primitive for Pointer
{
  const TypeTag: Type = Type::Pointer;

  /// Extracts the address from a [`Value::Pointer`].
  fn fromValue(value: Value) -> Result<Self, FFIError>
  {
    match value
    {
      Value::Pointer(addr) => Ok(Self(addr)),
      _ => Err(FFIError::Other(format!("expected Pointer, got {:?}", value))),
    }
  }

  /// Converts this [`Pointer`] wrapper into a [`Value::Pointer`].
  fn toValue(self) -> Value 
  {
    Value::Pointer(self.0)
  }
}

impl std::fmt::UpperHex for Pointer
{
  /// Formats the pointer address using uppercase hexadecimal notation.
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
  {
    std::fmt::UpperHex::fmt(&self.0, f)
  }
}

// =================================================================================================

impl From<Pointer> for usize
{
  /// Extracts the underlying `usize` memory address from a [`Pointer`].
  fn from(p: Pointer) -> Self { p.0 }
}

impl From<Pointer> for Value
{
  /// Converts a [`Pointer`] directly into a [`Value::Pointer`] variant.
  fn from(p: Pointer) -> Self { Self::Pointer(p.0) }
}

// =================================================================================================