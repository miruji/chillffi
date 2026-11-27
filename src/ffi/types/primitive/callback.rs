
// =================================================================================================

/// Handle of a closure registered in the clone's callback registry.
///
/// The `u64` is the registry ID — passed to C, it acts as a function pointer.
///
/// Derives mirror [`Pointer`](super::Pointer): the handle is a plain ID — hosts that keep
/// callbacks as objects (`rts`, `chillffi-py`) clone it into every call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Callback(pub(crate) u64);

impl Callback
{
  /// Registry ID of the `callback` (diagnostics, host-side bookkeeping).
  pub const fn id(&self) -> u64
  {
    self.0
  }
}

// =================================================================================================

// todo It is not clear whether it is worth merging it with the callback branch.
//  In theory, they are related and should be together.
