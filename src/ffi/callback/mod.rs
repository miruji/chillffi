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
use crate::ffi::types::primitive::Primitive;
use serde::Serialize;
// =================================================================================================

/// Re-exported so macro-generated code can reach these without requiring the
/// call site to have `serde`/`bincode`/`serde_closure` directly in scope.
#[doc(hidden)]
pub mod __reexport
{
  pub use bincode;
  pub use serde;
  pub use serde_closure;
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
  let decodeFn: DecodeFn = unsafe { std::mem::transmute(absoluteAddr) };
  decodeFn(envelope.siteTag, envelope.argsOutputTag, &envelope.bytes)
}

// =================================================================================================

/// Wraps a closure so it can cross the zygote fork.
///
/// New syntax without an explicit capture list — variables from the surrounding
/// environment are captured automatically via `serde_closure::Fn!`. All captured
/// values must implement `Serialize + DeserializeOwned + Clone + 'static`.
///
/// The expansion still generates a per-call-site decode function (so the relative
/// offset mechanism continues to work) and monomorphizes it for the concrete
/// unnameable type produced by `serde_closure`.
#[macro_export]
macro_rules! callback
{
  ($scope:expr, |$($argName:ident : $argTy:ty),* $(,)?| -> $retTy:ty $body:block) =>
  {
    {
      // Build a serializable closure that automatically captures the environment.
      let closure = $crate::ffi::callback::__reexport::serde_closure::Fn!(
        move |$($argName: $argTy),*| -> $retTy { $body }
      );

      // Force monomorphization of a decode function for the exact type of
      // `closure`. The returned function pointer is what we store as relativeOffset.
      fn force_decode<F>() -> fn(u64, u64, &[u8]) -> ::std::result::Result<
        $crate::ffi::callback::ErasedCallable,
        $crate::ffi::callback::CallError
      >
      where
        F: $crate::ffi::callback::__reexport::serde::de::DeserializeOwned
          + Clone + Send + 'static
          + $crate::ffi::callback::__reexport::serde_closure::traits::Fn<
              ($($argTy,)*),
              Output = $retTy
            >,
      {
        fn decode_impl<F>(
          siteTag: u64,
          argsOutputTag: u64,
          bytes: &[u8]
        ) -> ::std::result::Result<
          $crate::ffi::callback::ErasedCallable,
          $crate::ffi::callback::CallError
        >
        where
          F: $crate::ffi::callback::__reexport::serde::de::DeserializeOwned
            + Clone + Send + 'static
            + $crate::ffi::callback::__reexport::serde_closure::traits::Fn<
                ($($argTy,)*),
                Output = $retTy
              >,
        {
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

          let (state, _): (F, usize) =
            $crate::ffi::callback::__reexport::bincode::serde::decode_from_slice(
              bytes,
              $crate::ffi::callback::__reexport::bincode::config::standard()
            ).map_err(|e| $crate::ffi::callback::CallError::Decode(
              ::std::string::ToString::to_string(&e)
            ))?;

          // Rebuild the typed entry that knows how to pull args from DynamicList
          // and call the deserialized closure via serde_closure::traits::Fn::call.
          fn call_typed<F>(
            state: &F,
            args: &$crate::ffi::types::primitive::DynamicList
          ) -> $retTy
          where
            F: $crate::ffi::callback::__reexport::serde_closure::traits::Fn<
                ($($argTy,)*),
                Output = $retTy
              >,
          {
            let mut __i: usize = 0;
            $(
              let $argName: $argTy = args
                .get(__i)
                .expect(concat!("callback arg ", stringify!($argName), ": expected ", stringify!($argTy)));
              __i += 1;
            )*
            // Always build a proper tuple (trailing comma makes 1-arg into (x,)).
            $crate::ffi::callback::__reexport::serde_closure::traits::Fn::call(
              state,
              ( $($argName,)* )
            )
          }

          ::std::result::Result::Ok(
            $crate::ffi::callback::ErasedCallable::fromStateAndFn(state, call_typed::<F>)
          )
        }
        decode_impl::<F>
      }

      // Force the concrete unnameable type of `closure` into the generic.
      let _force: fn(u64, u64, &[u8]) -> _ = {
        fn force<F>(_: &F) -> fn(u64, u64, &[u8]) -> ::std::result::Result<
          $crate::ffi::callback::ErasedCallable,
          $crate::ffi::callback::CallError
        >
        where
          F: $crate::ffi::callback::__reexport::serde::de::DeserializeOwned
            + Clone + Send + 'static
            + $crate::ffi::callback::__reexport::serde_closure::traits::Fn<
                ($($argTy,)*),
                Output = $retTy
              >,
        {
          force_decode::<F>()
        }
        force(&closure)
      };

      let siteTag: u64 = $crate::ffi::callback::addressing::tagOf(
        concat!(file!(), ":", line!(), ":", column!())
      );
      let relativeOffset: usize = $crate::ffi::callback::addressing::relativeOffsetOf(
        _force as *const () as usize
      );
      let argTypes: ::std::vec::Vec<$crate::ffi::types::Type> =
        vec![ $( <$argTy as $crate::ffi::types::primitive::Primitive>::TypeTag ),* ];
      let returnType: $crate::ffi::types::Type =
        <$retTy as $crate::ffi::types::primitive::Primitive>::TypeTag;

      $scope.callback(
        $crate::ffi::callback::sendable::Sendable::from_closure(
          closure,
          relativeOffset,
          siteTag,
          argTypes,
          returnType,
        )
      )
    }
  };
}

// =================================================================================================
