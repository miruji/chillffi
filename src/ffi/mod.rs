//! Basis: [`Library`](library::Library),
//! [`Scope`](scope::Scope),
//! [`AllocatedMemory`](allocatedMemory::AllocatedMemory),
//! and error types.
// =================================================================================================
pub mod library;
pub mod types;
pub mod errors;
pub mod allocatedMemory;
pub mod scope;
#[doc(hidden)]
pub mod callback;
// =================================================================================================
