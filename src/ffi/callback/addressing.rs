use fxhash::FxHasher;
use crate::ffi::callback::Type;
use std::hash::Hasher;
use std::hash::Hash;
// =================================================================================================

/// Base load address of the binary containing this very function.
#[doc(hidden)]
pub fn moduleBase() -> usize
{
  let mut info: libc::Dl_info = unsafe{ std::mem::zeroed() };
  unsafe{ libc::dladdr(moduleBase as *const () as *const libc::c_void, &mut info) };
  info.dli_fbase as usize
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
  let mut hasher: FxHasher = fxhash::FxHasher::default();
  sourceLocation.hash(&mut hasher);
  hasher.finish()
}

/// Deterministic hash of the argument/return [`Type`]s a callback was
/// declared with. Both sides of the wire compute it independently — the
/// sender in [`Sendable::encode`], the receiver inside the macro-generated
/// decode function — so an Args/Output mismatch is caught before the
/// captured state is deserialized.
///
/// Replaces the old `argsOutputTagOf::<Args, Output>()` (it hashed
/// `type_name`s of generic parameters that no longer exist at the decode
/// site now that `decode` is type-erased).
#[doc(hidden)]
pub fn typesTagOf(argTypes: &[Type], returnType: &Type) -> u64
{
  let mut hasher: FxHasher = FxHasher::default();
  argTypes.hash(&mut hasher);
  returnType.hash(&mut hasher);
  hasher.finish()
}

// =================================================================================================