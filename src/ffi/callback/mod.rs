pub mod sendable;
pub mod addressing;
// =================================================================================================
mod erased;
pub use erased::ErasedCallable;
// =================================================================================================
mod error;
pub use error::CallError;
// =================================================================================================
mod envelope;
pub(crate) use envelope::Envelope;
// =================================================================================================
use crate::ffi::callback::addressing::resolveRelative;
use crate::ffi::types::primitive::DynamicList;
use crate::ffi::types::Type;
use crate::ffi::types::Value;
use crate::ffi::types::primitive::{Primitive};
use serde::{Serialize};
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

/// Object-safe equivalent of `Fn(Args) -> Output`, callable through a trait object.
///
/// Inside this crate it has exactly one instantiation that matters:
/// `Callable<CallbackArgs, Value>` — the fully dynamic form the clone's
/// dispatcher holds. Macro-generated code never implements it directly: the
/// expansion runs in *foreign* crates where `Value` (`pub(crate)`) cannot
/// even be named; the bridge from the typed macro-generated entry point to
/// this dynamic form is `ErasedCallable` + `StateFnAdapter`.
pub trait Callable<Args, Output>: Send
{
  /// Executes the captured closure with the provided arguments.
  fn call(&self, args: Args) -> Output;
}

// =================================================================================================

/// Decodes bytes produced by [`Sendable::encode`] into a callable object.
/// Called inside the zygote clone after receiving the bytes over IPC —
/// requires no startup registration of any kind in that process.
///
/// Not generic over `Args`/`Output` any more: the fn pointer it transmutes
/// to is generated in a foreign crate and must have a *nameable* signature,
/// so the erased [`ErasedCallable`] is the return type.
pub fn decode(bytes: &[u8]) -> Result<ErasedCallable, CallError>
{
  let (envelope, _): (Envelope, usize) = bincode::serde::decode_from_slice(bytes, bincode::config::standard())
    .map_err(|e| CallError::Decode(e.to_string()))?;

  type DecodeFn = fn(u64, u64, &[u8]) -> Result<ErasedCallable, CallError>;

  let absoluteAddr: usize = resolveRelative(envelope.relativeOffset);
  // Safety: `absoluteAddr` was produced by `relativeOffsetOf` from a valid
  // `fn` item pointer in this exact executable, transmitted, and resolved back.
  //
  // Soundness relies strictly on both processes running the identical binary file
  // (zygote guarantees this via re-exec of `current_exe()`).
  //
  // As a second line of defense, the target function re-checks both the
  // call-site tag and the argument/return type tag before it deserializes
  // anything.
  let decodeFn: DecodeFn = unsafe{ std::mem::transmute(absoluteAddr) };
  decodeFn(envelope.siteTag, envelope.argsOutputTag, &envelope.bytes)
}

// =================================================================================================

/// Wraps a closure so it can cross the zygote fork:
/// `callback!([x: i32] |args: T| -> U { ... })`.
///
/// Captures must be listed explicitly and their types must impl `Clone +
/// Serialize + DeserializeOwned` — `call` takes `&self`, not `self` (the same
/// closure can be invoked any number of times, e.g. once per comparison in a
/// `qsort`), so the captured tuple is cloned out on each call rather than moved.
///
/// The expansion never mentions `Value` (a `pub(crate)` type, unnameable in
/// foreign crates) and needs neither `serde` nor `bincode` at the call site:
/// captures travel as a plain tuple (serde has blanket impls for those), and
/// bincode is reached through `__reexport`.
#[macro_export]
macro_rules! callback
{
  ($scope:expr, [$($name:ident : $ty:ty),* $(,)?] |$($argName:ident : $argTy:ty),* $(,)?| -> $retTy:ty $body:block) =>
  {
    {
      // The macro generates a typed function that works with the captured
      // state and dynamically typed arguments.
      // It also generates the decoder that reconstructs this function later.
      
      // Typed entry point: extracts the declared arguments from the dynamic
      // `CallbackArgs` by position and runs the original body. Only public
      // API is touched here (`CallbackArgs::get`, `Primitive::TypeTag`).
      #[allow(unused_variables, unused_mut)]
      fn __callTyped(
        state: &($($ty,)*),
        args: &$crate::ffi::types::primitive::DynamicList
      ) -> $retTy
      {
        // Clone the captured tuple out — the same closure may run many times.
        let ($($name,)*) = state.clone();

        // Read each argument from the dynamic list and convert it to its
        // declared Rust type using the `Primitive` implementation.
        let mut __i: usize = 0;
        $(
          let $argName: $argTy = args
            .get(__i)
            .expect(concat!("callback arg ", stringify!($argName), ": expected ", stringify!($argTy)));
          __i += 1;
        )*

        let result: $retTy = { $body };
        result
      }

      // This function is stored in the sendable callback and later resolved
      // in the clone back into the original typed callable.
      // Signature must exactly match `DecodeFn` in `decode`:
      // fn(u64 /*siteTag*/, u64 /*argsOutputTag*/, &[u8]) -> Result<ErasedCallable, CallError>.
      // No `Value` anywhere — that is what makes it compilable in foreign crates.
      fn __callDecode(
        siteTag: u64,
        argsOutputTag: u64,
        bytes: &[u8]
      ) -> ::std::result::Result<
        $crate::ffi::callback::ErasedCallable,
        $crate::ffi::callback::CallError
      >
      {
        // Rebuild the same tags at the destination and verify that the
        // callback matches the call site and its argument/return types.
        let expectedSiteTag: u64 = $crate::ffi::callback::addressing::tagOf(
          concat!(file!(), ":", line!(), ":", column!())
        );
        if siteTag != expectedSiteTag {
          return ::std::result::Result::Err(
            $crate::ffi::callback::CallError::TypeMismatch { tag: siteTag }
          );
        }

        let expectedTypesTag: u64 = $crate::ffi::callback::addressing::typesTagOf(
          &[ $( <$argTy as $crate::ffi::types::primitive::Primitive>::TypeTag ),* ],
          &<$retTy as $crate::ffi::types::primitive::Primitive>::TypeTag
        );
        if argsOutputTag != expectedTypesTag {
          return ::std::result::Result::Err(
            $crate::ffi::callback::CallError::ArgsOutputMismatch
          );
        }

        // Deserialize the captured state that was serialized when the
        // callback was created.
        let (state, _): (($($ty,)*), usize) =
          $crate::ffi::callback::__reexport::bincode::serde::decode_from_slice(
            bytes,
            $crate::ffi::callback::__reexport::bincode::config::standard()
          ).map_err(|e| $crate::ffi::callback::CallError::Decode(
            ::std::string::ToString::to_string(&e)
          ))?;

        // Combine the restored state with the typed entry point into the
        // erased callable used by the internal dispatcher.
        ::std::result::Result::Ok(
          $crate::ffi::callback::ErasedCallable::fromStateAndFn(state, __callTyped)
        )
      }

      // Prepare the metadata and function address needed to send the
      // callback through the zygote boundary.
      let siteTag: u64 = $crate::ffi::callback::addressing::tagOf(
        concat!(file!(), ":", line!(), ":", column!())
      );
      let relativeOffset: usize = $crate::ffi::callback::addressing::relativeOffsetOf(
        __callDecode as *const () as usize
      );
      let argTypes: ::std::vec::Vec<$crate::ffi::types::Type> =
        vec![ $( <$argTy as $crate::ffi::types::primitive::Primitive>::TypeTag ),* ];
      let returnType: $crate::ffi::types::Type =
        <$retTy as $crate::ffi::types::primitive::Primitive>::TypeTag;

      // Serialize the captured state together with the callback metadata
      // and hand everything to the scope so the clone can invoke it later.
      $scope.callback(
        $crate::ffi::callback::sendable::Sendable::new(
          relativeOffset,
          siteTag,
          argTypes,
          returnType,
          ($($name,)*),
          __callTyped
        )
      )
      //
    }
  };
  //
}

// =================================================================================================