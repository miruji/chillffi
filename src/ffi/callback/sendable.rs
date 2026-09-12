use crate::ffi::callback::addressing::typesTagOf;
use crate::ffi::callback::CallError;
use crate::ffi::callback::DynamicList;
use crate::ffi::callback::Envelope;
use crate::ffi::callback::Primitive;
use crate::ffi::callback::Type;
use serde::Serialize;
// =================================================================================================

/// A concrete closure produced by [`callback!`], still on the originating side.
///
/// Holds the serialized form of a `serde_closure::Fn` (automatic captures) together
/// with the metadata required to reconstruct and invoke it inside the zygote clone.
/// The relativeOffset points at the monomorphized decode function generated for this
/// particular closure type at the call site.
pub struct Sendable
{
  /// Offset to the corresponding auto-generated decode function.
  relativeOffset: usize,

  /// Source code location hash used for target verification.
  siteTag: u64,

  /// Serialized closure (produced by serde_closure + bincode).
  state: Vec<u8>,

  /// Argument types captured for target-side signature verification.
  pub(crate) argTypes: Vec<Type>,

  /// Return type captured for target-side signature verification.
  pub(crate) returnType: Type,
}

impl Sendable
{
  /// Builds a `Sendable` from a serializable closure produced by `serde_closure::Fn!`.
  ///
  /// The `relativeOffset` must be the address of the monomorphized `__callDecode`
  /// that knows how to deserialize this exact closure type and turn it into an
  /// `ErasedCallable`.
  #[doc(hidden)]
  pub fn from_closure<F>(
    closure: F,
    relativeOffset: usize,
    siteTag: u64,
    argTypes: Vec<Type>,
    returnType: Type,
  ) -> Self
  where
    F: Serialize + Clone + 'static,
  {
    let state: Vec<u8> = bincode::serde::encode_to_vec(&closure, bincode::config::standard())
      .expect("callback: failed to serialize closure");

    Self {
      relativeOffset,
      siteTag,
      state,
      argTypes,
      returnType,
    }
  }

  /// Serializes everything needed to reconstruct and call this closure in
  /// the zygote clone.
  pub fn encode(&self) -> Result<Vec<u8>, CallError>
  {
    let envelope: Envelope = Envelope {
      relativeOffset: self.relativeOffset,
      argsOutputTag: typesTagOf(&self.argTypes, &self.returnType),
      siteTag: self.siteTag,
      bytes: self.state.clone(),
    };

    bincode::serde::encode_to_vec(&envelope, bincode::config::standard())
      .map_err(|e| CallError::Encode(e.to_string()))
  }
}

// =================================================================================================
