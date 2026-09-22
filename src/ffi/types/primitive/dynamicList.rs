use crate::ffi::errors::FFIError;
use crate::ffi::types::primitive::{Arg, FfiPrimitive};
use crate::ffi::types::Value;
// =================================================================================================

/// Dynamic list of FFI values.
pub struct DynamicList
{
  /// FFI Values.
  values: Box<[Value]>
}

impl DynamicList
{
  /// Creates a wrapper from a vector of values.
  ///
  /// (due to [`Value`] being used only within the crate)
  pub(crate) const fn fromValues(values: Box<[Value]>) -> Self
  {
    Self { values }
  }

  /// Returns the number of fields in the structure.
  pub const fn len(&self) -> usize
  {
    self.values.len()
  }

  /// Checks whether the structure is empty.
  pub const fn isEmpty(&self) -> bool
  {
    self.values.is_empty()
  }

  /// Extracts a field by index and converts it into the required type `T`.
  pub fn get<T: FfiPrimitive>(&self, index: usize) -> Result<T, FFIError>
  {
    self.values
      .get(index)
      .ok_or_else(|| FFIError::Other(format!("field index {} out of bounds", index)))
      .and_then(|v| T::fromFfiValue(Arg(v.clone())))
  }

  /// Extracts a nested struct field by index as a [`StructValue`].
  ///
  /// The field must itself be a `Value::Struct`; a scalar or any other
  /// variant returns an error rather than panicking.
  pub fn getStruct(&self, index: usize) -> Result<StructValue, FFIError>
  {
    match self.values.get(index)
    {
      Some(Value::Struct(values)) => Ok(StructValue::fromValues(values.clone())),
      Some(other) => Err(FFIError::Other(format!(
        "field {index}: expected Struct, got {other:?}"
      ))),
      None => Err(FFIError::Other(format!("field index {index} out of bounds")))
    }
  }
}

// =================================================================================================

impl From<Box<[Value]>> for DynamicList
{
  /// Converts a vector of `Value`s into a [`DynamicList`].
  fn from(values: Box<[Value]>) -> Self
  {
    Self { values }
  }
}

// =================================================================================================

/// A by-value C structure: an ordered list of field values.
///
/// Pass to [`CallBuilder::arg`](crate::ffi::library::CallBuilder::arg) to
/// transfer a struct by value (registers or stack, according to the platform
/// ABI). Field types are inferred from the concrete [`Arg`] values — there is
/// no separate type schema on the argument side.
///
/// For results use [`CallBuilder::resultStruct`](crate::ffi::library::CallBuilder::resultStruct),
/// which needs an explicit `&[Type]` layout because the return buffer is
/// untyped until decoded.
#[derive(Debug, Clone)]
pub struct StructValue
{
  /// Ordered field values (same representation as [`DynamicList`]).
  pub(crate) values: Box<[Value]>
}

impl StructValue
{
  /// Builds a by-value struct from a sequence of [`Arg`]s.
  ///
  /// Each argument becomes one field; nested structs are themselves
  /// `StructValue` values wrapped in `Arg::from(...)`.
  pub fn new(fields: impl IntoIterator<Item = Arg>) -> Self
  {
    Self {
      values: fields.into_iter().map(|a| a.0).collect::<Vec<_>>().into_boxed_slice()
    }
  }

  /// Creates a wrapper from a vector of values.
  ///
  /// (due to [`Value`] being used only within the crate)
  pub(crate) const fn fromValues(values: Box<[Value]>) -> Self
  {
    Self { values }
  }

  /// Returns the number of fields in the structure.
  pub const fn len(&self) -> usize
  {
    self.values.len()
  }

  /// Checks whether the structure is empty.
  pub const fn isEmpty(&self) -> bool
  {
    self.values.is_empty()
  }

  /// Extracts a field by index and converts it into the required type `T`.
  pub fn get<T: FfiPrimitive>(&self, index: usize) -> Result<T, FFIError>
  {
    self.values
      .get(index)
      .ok_or_else(|| FFIError::Other(format!("field index {} out of bounds", index)))
      .and_then(|v| T::fromFfiValue(Arg(v.clone())))
  }

  /// Extracts a nested struct field by index as a [`StructValue`].
  ///
  /// The field must itself be a `Value::Struct`; a scalar or any other
  /// variant returns an error rather than panicking.
  pub fn getStruct(&self, index: usize) -> Result<Self, FFIError>
  {
    match self.values.get(index)
    {
      Some(Value::Struct(values)) => Ok(Self::fromValues(values.clone())),
      Some(other) => Err(FFIError::Other(format!(
        "field {index}: expected Struct, got {other:?}"
      ))),
      None => Err(FFIError::Other(format!("field index {index} out of bounds")))
    }
  }

  /// Converts into a [`DynamicList`] (same field order, shared representation).
  pub fn intoList(self) -> DynamicList
  {
    DynamicList::fromValues(self.values)
  }
}

// =================================================================================================

impl From<StructValue> for DynamicList
{
  /// Converts a by-value struct into a dynamic field list.
  fn from(s: StructValue) -> Self
  {
    Self::fromValues(s.values)
  }
}

impl From<DynamicList> for StructValue
{
  /// Converts a dynamic field list into a by-value struct wrapper.
  fn from(list: DynamicList) -> Self
  {
    Self { values: list.values }
  }
}

// =================================================================================================
