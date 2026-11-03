use crate::ffi::types::primitive::{Callback, Pointer, StructValue};
use crate::ffi::types::Value;
use std::ffi::CStr;
use std::ffi::CString;
// =================================================================================================

/// Public wrapper over `Value` for signatures of public methods.
#[derive(Debug, Clone)]
pub struct Arg(pub(crate) Value);

#[doc(hidden)]
pub mod private
{
  use super::Arg;

  pub trait Sealed {}

  pub trait IntoFfiValue
  {
    /// Converts the type into its internal `Value` representation, wrapped in `Arg`.
    fn intoFfiValue(self) -> Arg;
  }
}

/// Sealed marker trait for types that can be passed into FFI calls.
///
/// Prevents external users from constructing or using `Value` directly.
pub trait FfiArg: private::Sealed + private::IntoFfiValue {}
#[doc(hidden)]
impl<T: private::Sealed + private::IntoFfiValue> FfiArg for T {}

// =================================================================================================

// Callback

impl From<Callback> for Arg
{
  /// Wraps the callback ID into an [`Arg`].
  fn from(callback: Callback) -> Self
  {
    Self(Value::Function(callback.0))
  }
}

impl private::Sealed for Callback {}

impl private::IntoFfiValue for Callback
{
  /// Converts the handle into an [`Arg`].
  fn intoFfiValue(self) -> Arg
  {
    Arg(Value::Function(self.0))
  }
}

// =================================================================================================

/// Implements `Sealed` (-> `FfiArg`) and `From<$type> for Arg` in one shot.
macro_rules! implSealedArg
{
  ($type:ty) =>
  {
    impl private::Sealed for $type {}

    impl private::IntoFfiValue for $type
    {
      /// Converts the value into an [`Arg`].
      fn intoFfiValue(self) -> Arg
      {
        Arg(Value::from(self))
      }
    }
    
    impl From<$type> for Arg
    {
      /// Wraps the value into an [`Arg`].
      fn from(v: $type) -> Self
      {
        Self(Value::from(v))
      }
    }
  };
}

// =================================================================================================

// Implementing conversions for all supported FFI argument types.

implSealedArg!(u8);
implSealedArg!(u16);
implSealedArg!(u32);
implSealedArg!(u64);
implSealedArg!(usize);
implSealedArg!(i8);
implSealedArg!(i16);
implSealedArg!(i32);
implSealedArg!(i64);
implSealedArg!(isize);
implSealedArg!(f32);
implSealedArg!(f64);
implSealedArg!(bool);
implSealedArg!(Pointer);
implSealedArg!(String);
implSealedArg!(&str);
implSealedArg!(CString);
implSealedArg!(&CStr);
implSealedArg!(Vec<u8>);
implSealedArg!(&[u8]);

// =================================================================================================

// A byte-string literal `b"..."` is `&[u8; N]`, and Rust does not coerce it
// to `&[u8]` inside a generic call - so it is accepted as is, without
// a manual `.to_vec()`.

impl<const N: usize> private::Sealed for &[u8; N] {}

impl<const N: usize> private::IntoFfiValue for &[u8; N]
{
  fn intoFfiValue(self) -> Arg
  {
    Arg(Value::from(self.as_slice()))
  }
}

impl<const N: usize> From<&[u8; N]> for Arg
{
  fn from(v: &[u8; N]) -> Self
  {
    Self(Value::from(v.as_slice()))
  }
}

// =================================================================================================

impl private::Sealed for StructValue {}

impl private::IntoFfiValue for StructValue
{
  fn intoFfiValue(self) -> Arg
  {
    Arg(Value::Struct(self.values))
  }
}

impl From<StructValue> for Arg
{
  fn from(s: StructValue) -> Self
  {
    Self(Value::Struct(s.values))
  }
}

// =================================================================================================
