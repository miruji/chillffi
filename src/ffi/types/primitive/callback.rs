
// =================================================================================================

/// Handle of a closure registered in the clone's callback registry.
///
/// The `u64` is the registry ID — passed to C, it acts as a function pointer.
pub struct Callback(pub(crate) u64);

// =================================================================================================

// todo It is not clear whether it is worth merging it with the callback branch.
//  In theory, they are related and should be together.
