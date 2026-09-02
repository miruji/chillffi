
// =================================================================================================

/// Handle of a closure registered in the clone's callback registry.
///
/// The `u64` is the registry ID — passed to C, it acts as a function pointer.
pub struct Callback(pub(crate) u64);

// =================================================================================================

// todo Не понятно, стоит ли его сливать с веткой callback.
//  По идее они связаны и должны быть вместе.