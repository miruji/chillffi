use serde::{Deserialize, Serialize};
// =================================================================================================

/// Wire format: a relative pointer to the concrete decode function, both
/// sanity tags, and the captured state's own serialized bytes.
#[derive(Serialize, Deserialize)]
#[doc(hidden)]
pub struct Envelope
{
  /// Offset of the target decode function relative to the module base.
  pub relativeOffset: usize,

  /// Hash of the argument and return types.
  pub argsOutputTag: u64,

  /// Hash of the source code location where the callback was defined.
  pub siteTag: u64,

  /// Serialized state of the captured variables.
  pub bytes: Vec<u8>
}

// =================================================================================================