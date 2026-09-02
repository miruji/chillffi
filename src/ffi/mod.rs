//! Basis: [`Library`](crate::ffi::library::Library),
//! [`Value`](crate::ffi::types::Value), [`Type`](crate::ffi::types::Type),
//! [`Scope`](crate::ffi::scope::Scope),
//! [`AllocatedMemory`](crate::ffi::allocatedMemory::AllocatedMemory),
//! and error types.

pub mod library;
pub mod types;
pub mod errors;
pub mod allocatedMemory;
pub mod scope;
pub mod callback;