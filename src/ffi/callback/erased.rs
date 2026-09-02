use crate::ffi::callback::Primitive;
use crate::ffi::callback::Value;
use crate::ffi::callback::Callable;
use crate::ffi::callback::DynamicList;
// =================================================================================================

/// The type-erased, dynamically callable form of a [`callback!`] closure —
/// what [`decode`] reconstructs inside the clone.
///
/// This is the public boundary of the otherwise `pub(crate)` dynamic world:
/// its constructor takes only nameable types (a state tuple + a typed fn
/// pointer), so macro-generated code in foreign crates can build it, while
/// actually *invoking* it (`ErasedCallable::call`, which does traffic in
/// `Value`) stays crate-internal. This is what allows `Value` to remain
/// `pub(crate)`.
pub struct ErasedCallable
{
  /// Type-erased callable implementation.
  inner: Box<dyn Callable<DynamicList, Value>>
}

impl ErasedCallable
{
  /// Wraps a decoded capture-state tuple plus the macro-generated typed
  /// entry point into the erased, dispatcher-facing callable.
  #[doc(hidden)]
  pub fn fromStateAndFn<State: Send + 'static, Output: Primitive + 'static>(
    state: State,
    typedFn: fn(&State, &DynamicList) -> Output
  ) -> Self
  {
    Self { inner: Box::new(StateFnAdapter { state, typedFn }) }
  }

  /// Invokes the erased closure with dynamic arguments and returns the
  /// dynamic result.
  /// 
  /// `pub(crate)`: only this crate's dispatcher (running inside the clone).
  pub(crate) fn call(&self, args: DynamicList) -> Value
  {
    self.inner.call(args)
  }
}

/// In-crate bridge from a macro-generated typed entry point to the dynamic
/// `Callable<CallbackArgs, Value>` object held by the dispatcher. 
/// 
/// The only place where the two worlds meet.
struct StateFnAdapter<State: Send + 'static, Output: Primitive + 'static>
{
  /// Captured closure state.
  state: State,

  /// Typed function entry point.
  typedFn: fn(&State, &DynamicList) -> Output
}

impl<State: Send + 'static, Output: Primitive + 'static> 
  Callable<DynamicList, Value> for StateFnAdapter<State, Output>
{
  fn call(&self, args: DynamicList) -> Value
  {
    // The typed entry point returns the closure's concrete return type;
    // convert it to the dynamic form the C-side marshalling understands.
    <Output as Primitive>::toValue((self.typedFn)(&self.state, &args))
  }
}

// =================================================================================================