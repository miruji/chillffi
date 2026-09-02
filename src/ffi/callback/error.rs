
// =================================================================================================

/// Errors from encoding, decoding, or resolving a sent closure.
#[derive(Debug)]
pub enum CallError
{
  /// The argument/return type tag carried in the envelope doesn't match the
  /// tag embedded in the resolved decode function — caught by the target
  /// function itself, before it deserializes anything.
  ArgsOutputMismatch,

  /// The resolved function's own embedded call-site tag doesn't match the
  /// one in the envelope — caught by the target function itself, before it
  /// deserializes anything. In practice this means the two processes are not
  /// actually running the same binary, which this module assumes never happens.
  TypeMismatch { tag: u64 },

  /// Serialization of the closure or envelope failed.
  Encode(String),
  /// Deserialization of the closure or envelope failed.
  Decode(String)
}

impl std::fmt::Display for CallError
{
  /// Formats the error using its debug representation.
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{:?}", self) }
}
impl std::error::Error for CallError {}

// =================================================================================================