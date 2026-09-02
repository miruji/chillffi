use crate::ffi::errors::FFIError;
use crate::ffi::types::primitive::Primitive;
use crate::ffi::types::Value;
// =================================================================================================

/// Dynamic list of FFI values.
pub struct DynamicList
{
  /// FFI Values.
  values: Vec<Value>
}

impl DynamicList 
{
  /// Creates a wrapper from a vector of values.
  /// 
  /// (due to [`Value`] being used only within the crate)
  pub(crate) fn fromValues(values: Vec<Value>) -> Self 
  {
    Self { values }
  }

  /// Returns the number of fields in the structure.
  pub fn len(&self) -> usize 
  {
    self.values.len()
  }

  /// Checks whether the structure is empty.
  pub fn isEmpty(&self) -> bool 
  {
    self.values.is_empty()
  }

  /// Extracts a field by index and converts it into the required type `T`.
  pub fn get<T: Primitive>(&self, index: usize) -> Result<T, FFIError> 
  {
    self.values
      .get(index)
      .ok_or_else(|| FFIError::Other(format!("field index {} out of bounds", index)))
      .and_then(|v| T::fromValue(v.clone()))
  }
}

// =================================================================================================

impl From<Vec<Value>> for DynamicList
{
  /// Converts a vector of [`Value`]s into a [`DynamicList`].
  fn from(values: Vec<Value>) -> Self
  {
    Self { values }
  }
}

// =================================================================================================