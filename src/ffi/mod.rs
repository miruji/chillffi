//! Basis: [`Library`](crate::ffi::library::Library),
//! [`Value`](crate::ffi::value::Value), [`Type`](crate::ffi::value::Type),
//! [`Scope`](crate::ffi::scope::Scope),
//! [`AllocatedMemory`](crate::ffi::allocatedMemory::AllocatedMemory),
//! and error types.

pub mod library;
pub mod value;
pub mod errors;
pub mod allocatedMemory;
pub mod scope;
pub mod callback;