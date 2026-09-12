use crate::ffi::callback::addressing::typesTagOf;
use crate::ffi::callback::CallError;
use crate::ffi::callback::DynamicList;
use crate::ffi::callback::Envelope;
use crate::ffi::callback::Type;
use serde::Serialize;
// =================================================================================================

/// A concrete closure produced by [`callback!`], still on the originating side.
pub struct Sendable {
    pub relative_offset: usize,
    pub site_tag: u64,
    pub argTypes: Vec<Type>,
    pub returnType: Type,
    // ✅ **Храним сериализованное замыкание вместо кортежа захваченных переменных**
    pub state: Vec<u8>,
}

impl Sendable {
    /// Создаёт `Sendable` из замыкания с автоматическим захватом.
    /// **Требования**:
    /// - Захваченные переменные должны реализовывать `Serialize + Deserialize<'static> + Clone`.
    /// - Замыкание должно быть обёрнуто через `serde_closure::Fn!` чтобы реализовывать Serialize.
    pub fn from_closure<F>(
        closure: F,
        site_tag: u64,
        arg_types: Vec<Type>,
        return_type: Type,
    ) -> Self
    where
        F: Serialize + Clone + 'static,
    {
        // 🔥 **Сериализуем замыкание через `bincode` (API 2.x)**
        let serialized_closure = bincode::serde::encode_to_vec(&closure, bincode::config::standard())
            .expect("Failed to serialize closure");

        // 🔥 **Генерируем уникальный `relativeOffset` для функции десериализации**
        // (в реальности нужно использовать `relativeOffsetOf` для сгенерированной функции)
        let relative_offset = 0; // ← Заглушка (нужно доработать)

        Self {
            relative_offset,
            site_tag,
            argTypes: arg_types,
            returnType: return_type,
            state: serialized_closure,
        }
    }

    /// Calls the closure directly, in this process. Equivalent to calling the
    /// original closure — this never touches IPC or pointer resolution.
    pub fn call(&self, _args: &DynamicList) {
        // Stub — original typed call is gone
        unimplemented!("call not implemented in new design")
    }

    /// Serializes everything needed to reconstruct and call this closure in
    /// the zygote clone.
    pub fn encode(&self) -> Result<Vec<u8>, CallError> {
        // Assemble the envelope with the type tag, site hash, and payload.
        let envelope: Envelope = Envelope {
            relativeOffset: self.relative_offset,
            argsOutputTag: typesTagOf(&self.argTypes, &self.returnType),
            siteTag: self.site_tag,
            bytes: self.state.clone(),
        };

        // Encode the envelope into the final byte payload.
        bincode::serde::encode_to_vec(&envelope, bincode::config::standard())
            .map_err(|e| CallError::Encode(e.to_string()))
    }
}

// =================================================================================================
