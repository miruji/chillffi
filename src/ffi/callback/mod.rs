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
// =================================================================================================

/// Re-exported so macro-generated code can reach these without requiring the
/// call site to have `serde`/`bincode` directly in scope.
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
  let decodeFn: DecodeFn = unsafe{ std::mem::transmute(absoluteAddr) };
  decodeFn(envelope.siteTag, envelope.argsOutputTag, &envelope.bytes)
}

// =================================================================================================

/// Wraps a closure so it can cross the zygote fork.
/// New syntax WITHOUT `[]` — automatic capture via serde_closure.
#[macro_export]
macro_rules! callback {
    // ✅ **Новый синтаксис БЕЗ `[]`**
    ($scope:expr, |$($argName:ident : $argTy:ty),* $(,)?| -> $retTy:ty $body:block) => {
        {
            // 🔥 **Автоматический захват переменных из окружения через serde_closure**
            let closure = $crate::ffi::callback::__reexport::serde_closure::Fn!(
                move |$($argName: $argTy),*| -> $retTy { $body }
            );

            // 🔥 **Сериализуем замыкание как единое целое**
            // (требует, чтобы все захваченные переменные реализовывали `Serialize + Deserialize`)
            let siteTag = $crate::ffi::callback::addressing::tagOf(concat!(file!(), ":", line!(), ":", column!()));
            let argTypes = vec![$(<$argTy as $crate::ffi::types::primitive::Primitive>::TypeTag),*];
            let returnType = <$retTy as $crate::ffi::types::primitive::Primitive>::TypeTag;

            // 🔥 **Новая функция для создания `Sendable` из замыкания**
            let sendable = $crate::ffi::callback::sendable::Sendable::from_closure(
                closure,
                siteTag,
                argTypes,
                returnType,
            );

            $scope.callback(sendable)
        }
    };
}

// =================================================================================================
