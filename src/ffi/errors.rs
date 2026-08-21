use serde::Deserialize;
use serde::Serialize;
// =================================================================================================

/// Errors occurring during library loading and call execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FFIError
{
  /// The global zygote in ZygoteState was not initialized.
  ZygoteNotInitialized,
  /// The call is executed outside the context of the ffi!{} macro.
  NoActiveZygoteScope,
  /// IPC communication failure with the zygote process.
  ZygoteCommunicationFailed(String),

  /// Failed to dynamically load the library (.so / .dll).
  LibraryLoadFailed { libraryPath: String, message: String },
  /// The requested library was not found in the registry by its ID.
  LibraryNotFound { libraryPath: String },

  /// The requested function/symbol was not found in the loaded library.
  SymbolNotFound { functionName: String },
  /// An invalid argument was passed (for example, Value::None).
  BadArgument(String),
  
  /// Data serialization error during IPC.
  EncodeFailed(String),
  /// Data deserialization error during IPC.
  DecodeFailed(String),

  /// Internal error of the FFI mechanism: storage/argsFfi out of sync
  /// (runtime bug, not an input data issue).
  ArgumentDowncastFailed(String),

  /// Other unclassified errors.
  Other(String)
}

impl std::fmt::Display for FFIError
{
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
  {
    write!(f, "{:?}", self)
  }
}

impl<E: std::error::Error + 'static> From<E> for FFIError
{
  fn from(err: E) -> Self {
    Self::Other(err.to_string())
  }
}

// =================================================================================================