use std::ffi::CStr;
use std::ffi::CString;
use crate::ffi::types::Value;
use crate::ffi::types::primitive::{Callback, Pointer};
// =================================================================================================

/// Public wrapper over [`Value`] for signatures of public methods.
#[derive(Debug, Clone)]
pub struct Arg(pub(crate) Value);

// =================================================================================================

mod private
{
  use crate::ffi::types::Value;

  pub trait Sealed
  {
    /// Converts the type into its internal `Value` representation.
    fn intoFfiValue(self) -> Value;
  }
}

/// Sealed marker trait for types that can be passed into FFI calls.
/// 
/// Prevents external users from constructing or using `Value` directly.
pub trait FfiArg: private::Sealed {}

impl<T: private::Sealed> FfiArg for T {}

/// Implements `Sealed` (-> `FfiArg`) and `From<$type> for Arg` in one shot.
macro_rules! implSealedArg
{
  ($type:ty) =>
  {
    impl private::Sealed for $type
    {
      /// Converts the value through `Value::from`.
      fn intoFfiValue(self) -> Value { Value::from(self) }
    }
    
    impl From<$type> for Arg
    {
      /// Wraps the value into an `Arg`.
      fn from(v: $type) -> Self { Self(Value::from(v)) }
    }
  };
}

impl From<Callback> for Arg
{
  /// Wraps the callback ID into [`Value::Function`].
  fn from(callback: Callback) -> Self { Self(Value::Function(callback.0)) }
}

// =================================================================================================

// Callback

impl private::Sealed for Callback
{
  /// Converts the handle into [`Value::Function`].
  fn intoFfiValue(self) -> Value { Value::Function(self.0) }
}

// =================================================================================================

// Implementing conversions for all supported FFI argument types
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