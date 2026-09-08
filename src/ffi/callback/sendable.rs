use crate::ffi::callback::Primitive;
use crate::ffi::callback::Serialize;
use crate::ffi::callback::Type;
use crate::ffi::callback::addressing::typesTagOf;
use crate::ffi::callback::Envelope;
use crate::ffi::callback::CallError;
use crate::ffi::callback::DynamicList;
// =================================================================================================

/// A concrete closure produced by [`callback!`], still on the originating side.
///
/// `State` is the tuple of captured variables, `Output` is the closure's
/// return type. Note what is *absent* from this signature: `Value`. The type
/// is therefore nameable — and [`Sendable::new`] callable — from
/// macro-generated code in foreign crates, where `Value` (`pub(crate)`)
/// cannot be referenced.
///
/// Call it directly with [`Sendable::call`] — no process boundary involved —
/// or [`Sendable::encode`] it to send across the zygote fork.
pub struct Sendable<State: Serialize + Send, Output: Primitive>
{
  /// Offset to the corresponding auto-generated decode function.
  relativeOffset: usize,

  /// Source code location hash used for target verification.
  siteTag: u64,
  
  /// The captured variables, as a tuple.
  state: State,

  /// Typed, same-process entry point into the closure body.
  typedFn: fn(&State, &DynamicList) -> Output,

  /// Argument types captured for target-side signature verification.
  pub(crate) argTypes: Vec<Type>,
  /// Return type captured for target-side signature verification.
  pub(crate) returnType: Type
}

impl<State: Serialize + Send, Output: Primitive> Sendable<State, Output>
{
  #[doc(hidden)]
  pub fn new(
    relativeOffset: usize,
    siteTag: u64,
    argTypes: Vec<Type>,
    returnType: Type,
    state: State,
    typedFn: fn(&State, &DynamicList) -> Output
  ) -> Self
  {
    Self { 
      relativeOffset, 
      siteTag, argTypes, 
      returnType, 
      state, 
      typedFn 
    }
  }

  /// Calls the closure directly, in this process. Equivalent to calling the
  /// original closure — this never touches IPC or pointer resolution.
  pub fn call(&self, args: &DynamicList) -> Output
  {
    (self.typedFn)(&self.state, args)
  }

  /// Serializes everything needed to reconstruct and call this closure in
  /// the zygote clone.
  pub fn encode(&self) -> Result<Vec<u8>, CallError>
  {
    // Serialize the captured closure state into bincode bytes.
    let bytes: Vec<u8> = bincode::serde::encode_to_vec(&self.state, bincode::config::standard())
      .map_err(|e| CallError::Encode(e.to_string()))?;

    // Assemble the envelope with the type tag, site hash, and payload.
    let envelope: Envelope = Envelope {
      relativeOffset: self.relativeOffset,
      argsOutputTag: typesTagOf(&self.argTypes, &self.returnType),
      siteTag: self.siteTag,
      bytes
    };

    // Encode the envelope into the final byte payload.
    bincode::serde::encode_to_vec(&envelope, bincode::config::standard())
      .map_err(|e| CallError::Encode(e.to_string()))
  }
}

// =================================================================================================
