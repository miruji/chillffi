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
#[cfg(test)]
mod tests;
// =================================================================================================
use crate::ffi::callback::addressing::resolveRelative;
use crate::ffi::types::primitive::DynamicList;
use crate::ffi::types::Type;
use crate::ffi::types::Value;
use crate::ffi::types::primitive::Primitive;
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

/// Restricts the `$scope` position in [`callback!`] to genuine `Scope<'g>`
/// values, so passing anything else is a compile error.
///
/// `macro_rules!` can't express `$scope: Scope<'g>` directly — `$scope:expr`
/// is a token substitution, not a generic parameter, so an unconstrained
/// `$scope.callback(...)` method call would accept *any* type with a
/// same-shaped method, checked only after expansion. Routing the call
/// through this sealed trait instead makes the macro require the bound
/// explicitly: `IsScope` can only be implemented inside this crate (via the
/// private `Sealed` supertrait), and only `Scope<'g>` does — so anything
/// else fails to compile right here, not silently "worked" by accident.
pub mod sealed
{
  mod private { pub trait Sealed { } }

  #[doc(hidden)]
  pub trait IsScope: private::Sealed
  {
    fn __registerCallback(
      &self,
      f: crate::ffi::callback::sendable::Sendable
    ) -> crate::ffi::types::primitive::Callback;
  }

  impl<'g> private::Sealed for crate::ffi::scope::Scope<'g> { }
  impl<'g> IsScope for crate::ffi::scope::Scope<'g>
  {
    fn __registerCallback(
      &self,
      f: crate::ffi::callback::sendable::Sendable
    ) -> crate::ffi::types::primitive::Callback
    {
      self.callback(f)
    }
  }
}

// =================================================================================================

/// Wraps a closure so it can cross the zygote fork.
///
/// New syntax without an explicit capture list — variables from the surrounding
/// environment are captured automatically, exactly like any ordinary Rust
/// closure. No `serde_closure` (or any other crate) is involved: a native
/// `move |...| { ... }` closure already captures whatever it references, all
/// on its own, that part needs no library at all.
///
/// The one real problem a library like `serde_closure` solves is different:
/// *serializing* the resulting closure, which has an unnameable,
/// compiler-generated type. This crate solves it more narrowly, in a way
/// that fits the zygote architecture specifically instead of the general
/// case: captured variables must be `Copy + Send + 'static` (so the closure
/// itself ends up `Copy`), and the closure's captured state is transmitted
/// as a raw bit-copy of its own memory rather than a field-by-field
/// serialization. That is sound *because*:
///   - `Copy` guarantees no `Drop` impl exists, so bit-duplicating it can
///     never cause a double-free/double-drop — this is the load-bearing
///     bound, not a convenience.
///   - the clone is not a different program on a different machine but a
///     fork/re-exec of this exact binary, so the concrete monomorphized
///     `F`'s layout is byte-for-byte identical on both ends.
/// This means captures are restricted to plain, stack-only data (numbers,
/// bools, pointers, `#[derive(Clone, Copy)]` structs, ...) — no `String`,
/// `Vec`, `Box`, or anything else that owns a heap allocation. That is a
/// real capability reduction versus the explicit-list `[]` design (which
/// only needed `Serialize + Clone`), traded for not depending on an
/// external capture-detecting proc-macro.
///
/// The expansion still generates a per-call-site decode function (so the
/// relative offset mechanism continues to work) and monomorphizes it for
/// the concrete unnameable closure type.
#[macro_export]
macro_rules! callback
{
  ($scope:expr, |$($argName:ident : $argTy:ty),* $(,)?| -> $retTy:ty $body:block) =>
  {
    $crate::ffi::callback::sealed::IsScope::__registerCallback(
      &$scope,
      $crate::callback!(@sendable |$($argName : $argTy),*| -> $retTy $body)
    )
  };

  // Builds the `Sendable` without registering it anywhere — no `$scope` in
  // this form, so there is nothing to fake. Exists so advanced/test code can
  // hold onto the encoded bytes themselves (e.g. to hand them to a transport
  // other than the immediate `Scope`, or to exercise `decode`'s own
  // tag-verification directly). The `$scope` form above is just this plus
  // registration; both compute the site tag identically because both do it
  // from *this exact* expansion, not a shared/relayed one.
  (@sendable |$($argName:ident : $argTy:ty),* $(,)?| -> $retTy:ty $body:block) =>
  {
    {
      // A plain Rust closure. It captures whatever free variables it
      // references entirely on its own — this line needs no macro help.
      let closure = move |$($argName: $argTy),*| -> $retTy { $body };

      // Force monomorphization of a decode function for the exact type of
      // `closure`. The returned function pointer is what we store as relativeOffset.
      fn forceDecode<F>() -> fn(u64, u64, &[u8]) -> ::std::result::Result<
        $crate::ffi::callback::ErasedCallable,
        $crate::ffi::callback::CallError
      >
      where
        F: Fn($($argTy),*) -> $retTy + Copy + Send + 'static,
      {
        fn decodeImpl<F>(
          siteTag: u64,
          argsOutputTag: u64,
          bytes: &[u8]
        ) -> ::std::result::Result<
          $crate::ffi::callback::ErasedCallable,
          $crate::ffi::callback::CallError
        >
        where
          F: Fn($($argTy),*) -> $retTy + Copy + Send + 'static,
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

          // No serde/bincode here: the captured environment travels as a
          // raw bit-copy (see `Sendable::fromClosure`) and is read back the
          // same way. Sound only because `F: Copy` (no Drop to double-run)
          // and both ends are the same compiled binary (identical layout).
          if bytes.len() != ::std::mem::size_of::<F>() {
            return ::std::result::Result::Err(
              $crate::ffi::callback::CallError::Decode(::std::format!(
                "callback state size mismatch: expected {} bytes, got {}",
                ::std::mem::size_of::<F>(),
                bytes.len()
              ))
            );
          }
          // Safety: length was just checked above; `read_unaligned` doesn't
          // require `bytes.as_ptr()` to satisfy F's alignment. `F: Copy`
          // makes this a valid duplicate, not a torn-out original.
          let state: F = unsafe{ ::std::ptr::read_unaligned(bytes.as_ptr().cast::<F>()) };

          // Rebuild the typed entry that knows how to pull args from
          // DynamicList and call the closure directly (`state(a, b, ...)`),
          // no trait-object/trait-method indirection needed.
          fn call_typed<F>(
            state: &F,
            args: &$crate::ffi::types::primitive::DynamicList
          ) -> $retTy
          where
            F: Fn($($argTy),*) -> $retTy,
          {
            let mut __i: usize = 0;
            $(
              let $argName: $argTy = args
                .get(__i)
                .expect(concat!("callback arg ", stringify!($argName), ": expected ", stringify!($argTy)));
              __i += 1;
            )*
            state($($argName),*)
          }

          ::std::result::Result::Ok(
            $crate::ffi::callback::ErasedCallable::fromStateAndFn(state, call_typed::<F>)
          )
        }
        decodeImpl::<F>
      }

      // Force the concrete unnameable type of `closure` into the generic.
      let _force: fn(u64, u64, &[u8]) -> _ = {
        fn force<F>(_: &F) -> fn(u64, u64, &[u8]) -> ::std::result::Result<
          $crate::ffi::callback::ErasedCallable,
          $crate::ffi::callback::CallError
        >
        where
          F: Fn($($argTy),*) -> $retTy + Copy + Send + 'static,
        {
          forceDecode::<F>()
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

      $crate::ffi::callback::sendable::Sendable::fromClosure(
        closure,
        relativeOffset,
        siteTag,
        argTypes,
        returnType
      )
    }
  };
}

// =================================================================================================