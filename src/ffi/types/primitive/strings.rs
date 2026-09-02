use std::ffi::CStr;
use std::ffi::CString;
use crate::ffi::errors::FFIError;
use crate::ffi::types::Value;
// =================================================================================================

impl From<String> for Value
{
  /// Converts a Rust [`String`] into [`Value::String`] (ptr + len).
  fn from(s: String) -> Self { Self::String(s.into_bytes()) }
}

impl From<&str> for Value
{
  /// Converts a string slice `&str` into [`Value::String`] (ptr + len).
  fn from(s: &str) -> Self { Self::String(s.as_bytes().to_vec()) }
}

impl From<CString> for Value
{
  /// Converts a [`CString`] into [`Value::CString`].
  fn from(c: CString) -> Self { Self::CString(c.into_bytes()) }
}

impl From<&CStr> for Value
{
  /// Converts a C-string literal (`c"hello"`) into [`Value::CString`].
  fn from(c: &CStr) -> Self { Self::CString(c.to_bytes().to_vec()) }
}

impl From<Vec<u8>> for Value
{
  /// Converts raw bytes `Vec<u8>` into [`Value::RawString`].
  fn from(v: Vec<u8>) -> Self { Self::RawString(v) }
}

impl From<&[u8]> for Value
{
  /// Converts a byte slice `&[u8]` into [`Value::RawString`].
  fn from(v: &[u8]) -> Self { Self::RawString(v.to_vec()) }
}

impl TryFrom<Value> for String
{
  type Error = FFIError;

  /// Attempts to convert a string [`Value`] into a Rust [`String`].
  fn try_from(value: Value) -> Result<Self, Self::Error>
  {
    let bytes: Vec<u8> = extractStringBytes(value)?;
    Self::from_utf8(bytes)
      .map_err(|e| FFIError::Other(format!("not valid UTF-8: {e}")))
  }
}

impl TryFrom<Value> for CString
{
  type Error = FFIError;

  /// Attempts to convert a string [`Value`] into a [`CString`].
  fn try_from(value: Value) -> Result<Self, Self::Error>
  {
    let bytes: Vec<u8> = extractStringBytes(value)?;
    Self::new(bytes)
      .map_err(|e| FFIError::Other(format!("interior NUL byte: {e}")))
  }
}

impl TryFrom<Value> for Vec<u8>
{
  type Error = FFIError;

  /// Extracts raw bytes from a string [`Value`].
  fn try_from(value: Value) -> Result<Self, Self::Error>
  {
    extractStringBytes(value)
  }
}

/// Helper function to extract raw byte vector from any string [`Value`] variant.
fn extractStringBytes(value: Value) -> Result<Vec<u8>, FFIError>
{
  match value
  {
    Value::String(b) | Value::CString(b) | Value::RawString(b) => Ok(b),
    _ => Err(FFIError::Other(format!("expected a string Value, got {:?}", value))),
  }
}

// =================================================================================================