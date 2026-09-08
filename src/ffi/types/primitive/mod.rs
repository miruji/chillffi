mod none;
mod strings;
// =================================================================================================
mod pointer;
pub use pointer::Pointer;
// =================================================================================================
mod dynamicList;
pub use dynamicList::DynamicList;
// =================================================================================================
pub mod arg;
pub use arg::Arg;
pub(crate) use arg::FfiArg;
// =================================================================================================
mod callback;
pub use callback::Callback;
// =================================================================================================
use crate::ffi::errors::FFIError;
use crate::ffi::types::{Value, Type};
// =================================================================================================

/// Bridges a concrete Rust primitive to its [`Type`] tag.
pub trait Primitive: Sized
{
  const TypeTag: Type;
}

/// Internal conversion between a concrete Rust primitive and its [`Value`].
pub(crate) trait PrimitiveValue: Primitive
{
  /// Converts a dynamic [`Value`] into a concrete primitive type.
  fn fromValue(value: Value) -> Result<Self, FFIError>;

  /// Converts this primitive into a dynamic [`Value`].
  fn toValue(self) -> Value;
}

// =================================================================================================

/// Mirrors `arg::private`: hides [`Value`]/[`PrimitiveValue`] behind the
/// already-public [`Arg`] wrapper, so the methods below never name a
/// `pub(crate)` type in their own signature.
pub mod private
{
  use super::{Arg, PrimitiveValue};
  use crate::ffi::errors::FFIError;

  pub trait Sealed {}
  impl<T: PrimitiveValue> Sealed for T {}

  pub trait FromFfiValue: Sized
  {
    /// Converts a dynamic [`Arg`] into a concrete primitive type.
    fn fromFfiValue(arg: Arg) -> Result<Self, FFIError>;

    /// Converts this primitive into a dynamic [`Arg`].
    fn toFfiValue(self) -> Arg;
  }

  impl<T: PrimitiveValue> FromFfiValue for T
  {
    fn fromFfiValue(arg: Arg) -> Result<Self, FFIError>
    {
      T::fromValue(arg.0)
    }
    
    fn toFfiValue(self) -> Arg
    {
      Arg(self.toValue())
    }
  }
}

/// Sealed marker trait for concrete types producible as an FFI call/read
/// result. Only crate-internal [`PrimitiveValue`] implementors satisfy it —
/// external crates cannot name `PrimitiveValue` to implement this either —
/// which keeps [`Value`] and [`PrimitiveValue`] out of the public API while
/// still letting `T: FfiPrimitive` appear in `pub fn` signatures.
pub trait FfiPrimitive: Primitive + private::Sealed + private::FromFfiValue {}
impl<T: PrimitiveValue> FfiPrimitive for T {}

// =================================================================================================

/// Declares a binding between a primitive and a [`Value`] type.
macro_rules! implFFIPrimitive
{
  ($rustType:ty, $variant:ident) =>
  {
    impl Primitive for $rustType
    {
      const TypeTag: Type = Type::$variant;
    }

    impl PrimitiveValue for $rustType
    {
      /// Parses the specific [`Value`] variant into this primitive type.
      fn fromValue(value: Value) -> Result<Self, FFIError>
      {
        match value {
          Value::$variant(v) => Ok(v),
          _ => Err(FFIError::Other(format!("expected {}, got {:?}", stringify!($variant), value))),
        }
      }

      /// Wraps this primitive value into its corresponding [`Value`] enum variant.
      fn toValue(self) -> Value
      {
        Value::$variant(self)
      }
    }
    
    impl From<$rustType> for Value
    {
      /// Converts the raw primitive into a dynamic [`Value`].
      fn from(v: $rustType) -> Self { Value::$variant(v) }
    }
  };
}

// =================================================================================================

// Declaration of all primitive types.

implFFIPrimitive!(u8, U8);
implFFIPrimitive!(u16, U16);
implFFIPrimitive!(u32, U32);
implFFIPrimitive!(u64, U64);
implFFIPrimitive!(usize, Usize);
implFFIPrimitive!(i8, I8);
implFFIPrimitive!(i16, I16);
implFFIPrimitive!(i32, I32);
implFFIPrimitive!(i64, I64);
implFFIPrimitive!(isize, Isize);
implFFIPrimitive!(f32, F32);
implFFIPrimitive!(f64, F64);
implFFIPrimitive!(bool, Bool);

// =================================================================================================