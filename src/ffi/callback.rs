use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use fxhash::FxHasher;
// =================================================================================================

/// Re-exported so macro-generated code can reach these without requiring the
/// call site to have `serde`/`bincode` directly in scope.
#[doc(hidden)]
pub mod __reexport
{
  pub use bincode;
  pub use serde;
}

// =================================================================================================

/// Implemented by every closure produced by [`callback!`]. Object-safe
/// equivalent of `Fn(Args) -> Output`, callable through a trait object.
pub trait Callable<Args, Output>: Send
{
  /// Executes the captured closure with the provided arguments.
  fn call(&self, args: Args) -> Output;
}

// =================================================================================================

/// Errors from encoding, decoding, or resolving a sent closure.
#[derive(Debug)]
pub enum CallError
{
  /// `Args`/`Output` requested at the call site don't match what the sender
  /// encoded — caught before the relative pointer is resolved at all.
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
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{:?}", self) }
}
impl std::error::Error for CallError {}

// =================================================================================================

/// Base load address of the binary containing this very function. Every
/// `callback!`-generated decode function and everything calling into this
/// module is part of the same statically-linked executable, so probing from
/// right here always resolves to the whole program's own base.
#[doc(hidden)]
pub fn moduleBase() -> usize
{
  let mut info: libc::Dl_info = unsafe{ std::mem::zeroed() };
  unsafe{ libc::dladdr(moduleBase as *const () as *const libc::c_void, &mut info) };
  info.dli_fbase as usize
}

/// Turns an absolute function pointer (in *this* process) into an offset
/// that resolves back to the same function in any other process running the
/// exact same executable file, regardless of ASLR — ASLR randomizes the load
/// base, not the binary's own internal layout.
#[doc(hidden)]
pub fn relativeOffsetOf(absoluteAddr: usize) -> usize
{
  absoluteAddr.wrapping_sub(moduleBase())
}

/// Resolves a previously computed relative offset back into an absolute function pointer.
fn resolveRelative(offset: usize) -> usize
{
  moduleBase().wrapping_add(offset)
}

/// Deterministic (no ASLR / process-random seed — `fxhash` is a fixed
/// algorithm, fixed seed) hash of a call-site source location into the tag
/// embedded in every envelope from that site.
#[doc(hidden)]
pub fn tagOf(sourceLocation: &str) -> u64
{
  let mut hasher: FxHasher = fxhash::FxHasher::default();
  sourceLocation.hash(&mut hasher);
  hasher.finish()
}

/// Hash of `(type_name::<Args>(), type_name::<Output>())` — a cheap sanity
/// check the caller of [`decode`] runs against itself, before touching the
/// resolved pointer at all.
fn argsOutputTagOf<Args: 'static, Output: 'static>() -> u64
{
  let mut hasher: FxHasher = fxhash::FxHasher::default();
  std::any::type_name::<Args>().hash(&mut hasher);
  std::any::type_name::<Output>().hash(&mut hasher);
  hasher.finish()
}

// =================================================================================================

/// Wire format: a relative pointer to the concrete decode function, both
/// sanity tags, and the captured state's own serialized bytes.
#[derive(Serialize, Deserialize)]
#[doc(hidden)]
pub(super) struct Envelope
{
  /// Offset of the target decode function relative to the module base.
  pub relativeOffset: usize,

  /// Hash of the expected argument and return types.
  pub argsOutputTag: u64,

  /// Hash of the source code location where the callback was defined.
  pub siteTag: u64,

  /// Serialized state of the captured variables.
  pub bytes: Vec<u8>
}

/// A concrete closure produced by [`callback!`], still on the originating
/// side. Call it directly with [`Sendable::call`] — no process boundary
/// involved — or [`Sendable::encode`] it to send across the zygote fork.
pub struct Sendable<Args, Output, T: Callable<Args, Output> + Serialize>
{
  /// Offset to the corresponding auto-generated decode function.
  relativeOffset: usize,

  /// Source code location hash used for target verification.
  siteTag: u64,

  /// The actual closure and its captured state.
  value: T,

  /// Phantom marker to retain the closure's signature types.
  _marker: std::marker::PhantomData<(Args, Output)>
}

impl<Args: 'static, Output: 'static, T: Callable<Args, Output> + Serialize> Sendable<Args, Output, T>
{
  #[doc(hidden)]
  pub const fn new(relativeOffset: usize, siteTag: u64, value: T) -> Self
  {
    Self { relativeOffset, siteTag, value, _marker: std::marker::PhantomData }
  }

  /// Calls the closure directly, in this process. Equivalent to calling the
  /// original closure — this never touches IPC or pointer resolution.
  pub fn call(&self, args: Args) -> Output
  {
    self.value.call(args)
  }

  /// Serializes everything needed to reconstruct and call this closure in
  /// the zygote clone.
  pub fn encode(&self) -> Result<Vec<u8>, CallError>
  {
    let bytes: Vec<u8> = bincode::serde::encode_to_vec(&self.value, bincode::config::standard())
      .map_err(|e| CallError::Encode(e.to_string()))?;
    let envelope: Envelope = Envelope {
      relativeOffset: self.relativeOffset,
      argsOutputTag: argsOutputTagOf::<Args, Output>(),
      siteTag: self.siteTag,
      bytes
    };
    bincode::serde::encode_to_vec(&envelope, bincode::config::standard())
      .map_err(|e| CallError::Encode(e.to_string()))
  }
}

/// Decodes bytes produced by [`Sendable::encode`] into a callable trait
/// object. Called inside the zygote clone after receiving the bytes over
/// IPC — requires no startup registration of any kind in that process.
pub fn decode<Args: 'static, Output: 'static>(bytes: &[u8]) -> Result<Box<dyn Callable<Args, Output>>, CallError>
{
  let (envelope, _): (Envelope, usize) = bincode::serde::decode_from_slice(bytes, bincode::config::standard())
    .map_err(|e| CallError::Decode(e.to_string()))?;

  if envelope.argsOutputTag != argsOutputTagOf::<Args, Output>() {
    return Err(CallError::ArgsOutputMismatch);
  }

  type DecodeFn<Args, Output> = fn(u64, &[u8]) -> Result<Box<dyn Callable<Args, Output>>, CallError>;

  let absoluteAddr: usize = resolveRelative(envelope.relativeOffset);
  // Safety: `absoluteAddr` was produced by `relativeOffsetOf` from a valid
  // `fn` item pointer in this exact executable, transmitted, and resolved back.
  //
  // Soundness relies strictly on both processes running the identical binary file
  // (chillffi's zygote guarantees this via re-exec of `current_exe()`). 
  //
  // As a second line of defense, the target function re-checks `siteTag` 
  
  // before any deserialization occurs.
  let decodeFn: DecodeFn<Args, Output> = unsafe{ std::mem::transmute(absoluteAddr) };
  decodeFn(envelope.siteTag, &envelope.bytes)
}

// =================================================================================================

/// Wraps a closure so it can cross the zygote fork:
/// `callback!([x: i32] |args: T| -> U { ... })`.
///
/// Captures must be listed explicitly and their types must impl `Clone +
/// Serialize + DeserializeOwned` — `call` takes `&self`, not `self` (the same
/// closure can be invoked any number of times, e.g. once per comparison in a
/// `qsort`), so captured fields are cloned out on each call rather than moved.
#[macro_export]
macro_rules! callback
{
  ([$($name:ident : $ty:ty),* $(,)?] |$arg:ident : $argTy:ty| -> $retTy:ty $body:block) =>
  {
    {
      #[derive($crate::ffi::callback::__reexport::serde::Serialize, $crate::ffi::callback::__reexport::serde::Deserialize)]
      struct __CallImpl { $( $name: $ty, )* }

      impl $crate::ffi::callback::Callable<$argTy, $retTy> for __CallImpl
      {
        fn call(&self, $arg: $argTy) -> $retTy
        {
          $( let $name: $ty = self.$name.clone(); )*
          $body
        }
      }

      // Monomorphic, address-taken `fn` item — this address (relative to the
      // module base) is what actually crosses the wire; see `Sendable::new`
      // below. Checks its own site tag first, so a resolution that landed
      // here by mistake (only possible if the two processes are somehow not
      // the same binary) fails cleanly instead of deserializing garbage.
      fn __callDecode(siteTag: u64, bytes: &[u8]) -> ::std::result::Result<
        ::std::boxed::Box<dyn $crate::ffi::callback::Callable<$argTy, $retTy>>,
        $crate::ffi::callback::CallError
      >
      {
        let expected: u64 = $crate::ffi::callback::tagOf(::std::concat!(::std::file!(), ":", ::std::line!(), ":", ::std::column!()));
        if siteTag != expected {
          return ::std::result::Result::Err($crate::ffi::callback::CallError::TypeMismatch { tag: siteTag });
        }
        let (concrete, _): (__CallImpl, usize) = $crate::ffi::callback::__reexport::bincode::serde::decode_from_slice(
          bytes, $crate::ffi::callback::__reexport::bincode::config::standard()
        ).map_err(|e| $crate::ffi::callback::CallError::Decode(::std::string::ToString::to_string(&e)))?;
        ::std::result::Result::Ok(::std::boxed::Box::new(concrete))
      }

      let siteTag: u64 = $crate::ffi::callback::tagOf(::std::concat!(::std::file!(), ":", ::std::line!(), ":", ::std::column!()));
      let relativeOffset: usize = $crate::ffi::callback::relativeOffsetOf(__callDecode as usize);

      $crate::ffi::callback::Sendable::new(relativeOffset, siteTag, __CallImpl { $( $name, )* })
    }
  };
}

// =================================================================================================

#[cfg(test)]
mod tests
{
  use crate::ffi::callback::Envelope;
  use crate::ffi::callback::CallError;
  use crate::ffi::callback::decode;
  use crate::ffi::callback::Callable;
  // ===============================================================================================

/// Round-trips a closure through encode/decode within a single process.
  #[test]
  fn roundtrip() -> ()
  {
    let threshold: i32 = 5;
    let compar = callback!([threshold: i32] |args: Vec<i32>| -> i32 {
      args.iter().filter(|&&x| x > threshold).count() as i32
    });

    assert_eq!(compar.call(vec![1, 6, 9, 2]), 2);

    let bytes: Vec<u8> = compar.encode().expect("encode");
    let remote: Box<dyn Callable<Vec<i32>, i32>> = decode(&bytes).expect("decode");
    assert_eq!(remote.call(vec![1, 6, 9, 2]), 2);
  }

  /// Requesting the wrong Args/Output must fail cleanly — checked by the
  /// caller before any pointer resolution happens.
  #[test]
  fn argsOutputMismatchIsCaught() -> ()
  {
    let x: i32 = 1;
    let c = callback!([x: i32] |args: Vec<i32>| -> i32 { args.len() as i32 + x });
    let bytes: Vec<u8> = c.encode().expect("encode");

    let wrong: Result<Box<dyn Callable<String, bool>>, CallError> = decode(&bytes);
    assert!(matches!(wrong, Err(CallError::ArgsOutputMismatch)));
  }

  /// A resolved-but-wrong site (simulated by hand-corrupting the tag) must
  /// be caught by the target function itself, not silently produce garbage.
  #[test]
  fn siteTagMismatchIsCaught() -> ()
  {
    let x: i32 = 1;
    let c = callback!([x: i32] |args: Vec<i32>| -> i32 { args.len() as i32 + x });
    let mut bytes: Vec<u8> = c.encode().expect("encode");

    let (mut envelope, _): (Envelope, usize) = bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
    envelope.siteTag = envelope.siteTag.wrapping_add(1);
    bytes = bincode::serde::encode_to_vec(&envelope, bincode::config::standard()).unwrap();

    let result: Result<Box<dyn Callable<Vec<i32>, i32>>, CallError> = decode(&bytes);
    assert!(matches!(result, Err(CallError::TypeMismatch { .. })));
  }

  // ===============================================================================================
}

// =================================================================================================