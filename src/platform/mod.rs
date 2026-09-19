//! Platform layer.
//!
//! Currently exposes only [`ipc`], which abstracts the IPC between the
//! Runtime and the Zygote (Clone). Future platform-specific code
//! (file-system resolution, OS error mapping, ...) belongs here too.
// =================================================================================================

pub mod ipc;
pub mod low;

// =================================================================================================

/// Test-only re-export of the example platform helpers (`platformExt!`,
/// `LibmPath`, ...). The `examples/platform/mod.rs` source is included as
/// `crate::examplesPlatform` (see `lib.rs`); the tests reach those helpers
/// through `crate::platform::*` for backwards compatibility.
#[cfg(test)]
pub use crate::examplesPlatform::*;

// =================================================================================================
