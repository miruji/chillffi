use crate::ffi::callback::Type;
use fxhash::FxHasher;
use std::hash::Hash;
use std::hash::Hasher;
// =================================================================================================

/// Base load address of the binary containing this very function.
#[doc(hidden)]
pub fn moduleBase() -> usize
{
  crate::sys::moduleBase()
}

/// Turns an absolute function pointer (in *this* process) into an offset.
#[doc(hidden)]
pub fn relativeOffsetOf(absoluteAddr: usize) -> usize
{
  absoluteAddr.wrapping_sub(moduleBase())
}

/// Inverse of [`relativeOffsetOf`]: base + offset = absolute address.
pub(crate) fn resolveRelative(offset: usize) -> usize
{
  moduleBase().wrapping_add(offset)
}

/// Deterministic hash of a call-site source location.
#[doc(hidden)]
pub fn tagOf(sourceLocation: &str) -> u64
{
  let mut hasher: FxHasher = FxHasher::default();
  sourceLocation.hash(&mut hasher);
  hasher.finish()
}

/// Deterministic hash of the argument/return [`Type`]s a callback was
/// declared with. Both sides of the wire compute it independently — the
/// sender in [`Sendable::encode`], the receiver inside the macro-generated
/// decode function — so an Args/Output mismatch is caught before the
/// captured state is deserialized.
#[doc(hidden)]
pub fn typesTagOf(argTypes: &[Type], returnType: &Type) -> u64
{
  let mut hasher: FxHasher = FxHasher::default();
  argTypes.hash(&mut hasher);
  returnType.hash(&mut hasher);
  hasher.finish()
}

// =================================================================================================