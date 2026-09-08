use std::ffi::CStr;
use std::ffi::CString;
use crate::ffi::types::Value;
use crate::ffi::types::primitive::{Callback, Pointer};
// =================================================================================================

/// Public wrapper over [`Value`] for signatures of public methods.
#[derive(Debug, Clone)]
pub struct Arg(pub(crate) Value);

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

impl<T: private::Sealed + private::IntoFfiValue> FfiArg for T {}

// =================================================================================================

// Callback

impl From<Callback> for Arg
{
  /// Wraps the callback ID into [`Value::Function`].
  fn from(callback: Callback) -> Self
  {
    Self(Value::Function(callback.0))
  }
}

impl private::Sealed for Callback {}

impl private::IntoFfiValue for Callback
{
  /// Converts the handle into [`Value::Function`].
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
      /// Converts the value through `Value::from`.
      fn intoFfiValue(self) -> Arg
      {
        Arg(Value::from(self))
      }
    }
    
    impl From<$type> for Arg
    {
      /// Wraps the value into an `Arg`.
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