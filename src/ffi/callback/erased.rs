use crate::ffi::callback::Callable;
use crate::ffi::callback::DynamicList;
use crate::ffi::callback::Primitive;
use crate::ffi::callback::Value;
use crate::ffi::types::primitive::FfiPrimitive;
// =================================================================================================

/// The type-erased, dynamically callable form of a [`callback!`] closure —
/// what [`decode`] reconstructs inside the clone.
pub struct ErasedCallable
{
  /// Type-erased callable implementation.
  inner: Box<dyn Callable<DynamicList, Value>>
}

impl ErasedCallable
{
  /// Wraps a reconstructed closure (or any state + typed entry point pair)
  /// into the erased, dispatcher-facing callable.
  ///
  /// This is the only constructor [`ErasedCallable`] needs: a bit-copied
  /// native closure is just "some [`State`] plus a way to call it", exactly
  /// like any other [`State`] the macro could hand in, so there is no
  /// separate closure-specific path to maintain.
  #[doc(hidden)]
  pub fn fromStateAndFn<State: Send + 'static, Output: FfiPrimitive + 'static>(
    state: State,
    typedFn: fn(&State, &DynamicList) -> Output
  ) -> Self
  {
    Self {
      inner: Box::new(StateFnAdapter { state, typedFn })
    }
  }

  /// Invokes the erased closure with dynamic arguments and returns the
  /// dynamic result.
  pub(crate) fn call(&self, args: DynamicList) -> Value
  {
    self.inner.call(args)
  }
}

// =================================================================================================

/// In-crate bridge from a macro-generated typed entry point to the dynamic
/// `Callable<DynamicList, Value>` object held by the dispatcher.
///
/// The only place where the two worlds meet.
struct StateFnAdapter<State: Send + 'static, Output: Primitive + 'static>
{
  /// Captured closure state (reconstructed by bit-copy — see [`callback!`]).
  state: State,

  /// Typed function entry point.
  typedFn: fn(&State, &DynamicList) -> Output
}

impl<State: Send + 'static, Output: FfiPrimitive + 'static>
Callable<DynamicList, Value> for StateFnAdapter<State, Output>
{
  /// todo desc
  fn call(&self, args: DynamicList) -> Value
  {
    (self.typedFn)(&self.state, &args).toFfiValue().0
  }
}

// =================================================================================================