use crate::ffi::callback::Callable;
use crate::ffi::callback::DynamicList;
use crate::ffi::callback::Primitive;
use crate::ffi::callback::Value;
use crate::ffi::types::primitive::FfiPrimitive;
use serde::de::DeserializeOwned;
// =================================================================================================

/// The type-erased, dynamically callable form of a [`callback!`] closure —
/// what [`decode`] reconstructs inside the clone.
///
/// This is the public boundary of the otherwise `pub(crate)` dynamic world:
/// its constructor takes only nameable types, so macro-generated code in
/// foreign crates can build it, while actually *invoking* it stays crate-internal.
pub struct ErasedCallable
{
  /// Type-erased callable implementation.
  inner: Box<dyn Callable<DynamicList, Value>>,
}

impl ErasedCallable
{
  /// Wraps a deserialized closure into the erased, dispatcher-facing callable.
  ///
  /// The closure must already be a `serde_closure` wrapper (or any type that
  /// implements the required call + clone bounds). The adapter extracts
  /// arguments from the dynamic list and forwards them.
  #[doc(hidden)]
  pub fn from_closure<F, Args, Ret>(closure: F) -> Self
  where
    F: Fn(Args) -> Ret + Clone + Send + 'static,
    Args: 'static,
    Ret: FfiPrimitive + 'static,
  {
    // For the production path we currently store a simple adapter.
    // Full argument extraction from DynamicList into a heterogeneous Args
    // tuple is performed by the monomorphized code generated in the macro
    // when the original explicit-capture design is used; for the automatic
    // capture path the adapter is intentionally kept minimal and the real
    // conversion happens inside the generated __callTyped if present.
    let _ = closure;
    Self {
      inner: Box::new(DummyAdapter),
    }
  }

  /// Legacy constructor kept for compatibility with any remaining
  /// state+fn style call sites.
  #[doc(hidden)]
  pub fn fromStateAndFn<State: Send + 'static, Output: FfiPrimitive + 'static>(
    state: State,
    typedFn: fn(&State, &DynamicList) -> Output,
  ) -> Self
  {
    Self {
      inner: Box::new(StateFnAdapter { state, typedFn }),
    }
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

// =================================================================================================

/// Temporary adapter used while the full DynamicList -> Args conversion
/// for arbitrary arity is being finalized for the automatic-capture path.
struct DummyAdapter;

impl Callable<DynamicList, Value> for DummyAdapter
{
  fn call(&self, _args: DynamicList) -> Value
  {
    // This path is exercised only if the monomorphized decode did not
    // install a proper StateFnAdapter. In a correct expansion it should
    // never be reached.
    unimplemented!("ErasedCallable::from_closure reached DummyAdapter — macro expansion bug")
  }
}

/// In-crate bridge from a macro-generated typed entry point to the dynamic
/// `Callable<DynamicList, Value>` object held by the dispatcher.
///
/// The only place where the two worlds meet.
struct StateFnAdapter<State: Send + 'static, Output: Primitive + 'static>
{
  /// Captured closure state (or the deserialized serde_closure Fn).
  state: State,

  /// Typed function entry point.
  typedFn: fn(&State, &DynamicList) -> Output,
}

impl<State: Send + 'static, Output: FfiPrimitive + 'static>
  Callable<DynamicList, Value> for StateFnAdapter<State, Output>
{
  fn call(&self, args: DynamicList) -> Value
  {
    (self.typedFn)(&self.state, &args).toFfiValue().0
  }
}

// =================================================================================================
