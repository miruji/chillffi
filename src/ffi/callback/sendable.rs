use crate::ffi::callback::addressing::typesTagOf;
use crate::ffi::callback::CallError;
use crate::ffi::callback::Envelope;
use crate::ffi::callback::Type;
// =================================================================================================

/// A concrete closure produced by [`callback!`], still on the originating side.
///
/// Holds a raw bit-copy of a native closure's captured environment (see
/// [`Sendable::fromClosure`]) together with the metadata required to
/// reconstruct and invoke it inside the zygote clone. The relativeOffset
/// points at the monomorphized decode function generated for this
/// particular closure type at the call site.
pub struct Sendable
{
  /// Offset to the corresponding auto-generated decode function.
  relativeOffset: usize,

  /// Source code location hash used for target verification.
  siteTag: u64,

  /// Raw bytes of the closure's own in-memory representation.
  state: Vec<u8>,

  /// Argument types captured for target-side signature verification.
  pub(crate) argTypes: Vec<Type>,

  /// Return type captured for target-side signature verification.
  pub(crate) returnType: Type
}

impl Sendable
{
  /// Builds a `Sendable` from a plain, automatically-capturing Rust closure.
  ///
  /// The `relativeOffset` must be the address of the monomorphized decode
  /// function that knows how to reconstruct this exact closure type and
  /// turn it into an `ErasedCallable`.
  ///
  /// `F: Copy` is load-bearing, not a convenience bound: what follows is a
  /// raw bit-copy of `closure`'s memory, not a field-by-field serialization.
  /// That is only sound because `Copy` guarantees `F` has no `Drop` impl —
  /// duplicating its bytes can never run a destructor twice or leave one
  /// side pointing at memory the other side already freed. It also means
  /// captures are limited to plain, heap-free data (numbers, bools,
  /// pointers, `#[derive(Clone, Copy)]` structs, ...) — a `String` or `Vec`
  /// capture will fail to compile here, not corrupt memory at runtime.
  #[doc(hidden)]
  pub fn fromClosure<F>(
    closure: F,
    relativeOffset: usize,
    siteTag: u64,
    argTypes: Vec<Type>,
    returnType: Type,
  ) -> Self
  where
    F: Copy + Send + 'static,
  {
    // Safety: `closure` is `Copy`, so it has no `Drop` impl — reading its
    // representation out as bytes and letting the original also drop
    // normally can never double-free or double-run a destructor. The clone
    // reconstructs `F` from these same bytes (see the `callback!` macro),
    // which is valid because it is the identical compiled type, not a
    // foreign/portable format.
    let state: Vec<u8> = unsafe
      {
        std::slice::from_raw_parts(
          (&closure as *const F).cast::<u8>(),
          std::mem::size_of::<F>()
        )
      }.to_vec();

    Self {
      relativeOffset,
      siteTag,
      state,
      argTypes,
      returnType
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
      bytes: self.state.clone()
    };

    bincode::serde::encode_to_vec(&envelope, bincode::config::standard())
      .map_err(|e| CallError::Encode(e.to_string()))
  }
}

// =================================================================================================