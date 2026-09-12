use crate::ffi::callback::Callable;
use crate::ffi::callback::DynamicList;
use crate::ffi::callback::Primitive;
use crate::ffi::callback::Value;
use crate::ffi::types::primitive::FfiPrimitive;
use serde::de::DeserializeOwned;
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
    /// Создаёт `ErasedCallable` из **десериализованного замыкания**.
    pub fn fromStateAndFn<F>(
        state: Vec<u8>,  // ← Было: `state: ($($ty,)*)`
        _call_typed: fn(&F, &DynamicList) -> (),
    ) -> Self
    where
        F: DeserializeOwned + Clone + 'static,
    {
        // 🔥 **Десериализуем замыкание**
        let (closure, _): (F, usize) = bincode::serde::decode_from_slice(&state, bincode::config::standard())
            .expect("Failed to deserialize closure");

        // 🔥 **Создаём `ErasedCallable`, который вызывает замыкание**
        // (заглушка — реальный вызов требует знания Args/Ret)
        let _ = closure;
        Self {
            inner: Box::new(DummyAdapter)
        }
    }

    /// Новый метод для создания из замыкания напрямую.
    pub fn from_closure<F>(closure: F) -> Self
    where
        F: Clone + Send + 'static,
    {
        let _ = closure;
        Self {
            // 🔥 **Храним замыкание в `Box<dyn Callable>`** (заглушка)
            inner: Box::new(DummyAdapter)
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

/// Dummy adapter that satisfies the Callable trait (stub from the plan).
struct DummyAdapter;

impl Callable<DynamicList, Value> for DummyAdapter {
    fn call(&self, _args: DynamicList) -> Value {
        unimplemented!("from_closure / DummyAdapter is a stub from the plan")
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

impl<State: Send + 'static, Output: FfiPrimitive + 'static>
Callable<DynamicList, Value> for StateFnAdapter<State, Output>
{
  fn call(&self, args: DynamicList) -> Value
  {
    // The typed entry point returns the closure's concrete return type;
    // convert it to the dynamic form the C-side marshalling understands.
    (self.typedFn)(&self.state, &args).toFfiValue().0
  }
}

// =================================================================================================
