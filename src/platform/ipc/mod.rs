//! Transport: platform-specific IPC between Runtime and Zygote (Clone).
//!
//! Each backend implements the same [`Transport`] trait, so [`crate::zygote`]
//! stays platform-neutral:
//!
//! - **Linux**: [`self::linux`] — `ipc-channel` (`SCM_RIGHTS` sockets under
//!   the hood). `fork` inherits descriptors, so the data channels of a clone
//!   are created before the fork and need no workaround.
//! - **macOS**: [`self::macos`] — `ipc-channel` (Mach ports under the hood;
//!   the data channels have to be created by the clone after the fork).
//! - **Windows**: [`self::windows`] — named pipes (handles do not survive
//!   `RtlCloneUserProcess`, so `ipc-channel` is only used to hand over pipe
//!   names through the control channel).
//!
//! The IPC payload ([`FFIRequest`] / [`FFIResponse`]) is the same on every
//! backend: `serde` on top of whatever the backend transports.
// =================================================================================================
use crate::ffi::errors::FFIError;
use crate::ffi::types::{Type, Value};
use fxhash::FxHashMap;
use libloading::Library;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io;
use std::process::Child;
// =================================================================================================

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use self::linux::Bootstrap as LinuxBootstrap;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::Bootstrap as MacosBootstrap;

#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use self::windows::Bootstrap as WindowsBootstrap;

// Convenience alias used by [`crate::zygote`] to pick the active platform's
// `Bootstrap`. Per-platform aliases (`LinuxBootstrap`, `MacosBootstrap`,
// `WindowsBootstrap`) are re-exported above.
// Marked `allow(dead_code)` because only the platform-specific branch is
// consumed in `zygote.rs`; the inactive branches would otherwise trip
// `-D warnings`.
#[allow(dead_code)]
#[cfg(target_os = "linux")]
pub type Bootstrap = LinuxBootstrap;
#[allow(dead_code)]
#[cfg(target_os = "macos")]
pub type Bootstrap = MacosBootstrap;
#[allow(dead_code)]
#[cfg(windows)]
pub type Bootstrap = WindowsBootstrap;

// =================================================================================================

/// Hidden startup flag: if it is the first argument —
/// this is not the runtime, but the zygote process.
pub const ZygoteFlag: &str = "__zygote";

// =================================================================================================

/// Request for FFI execution, sent entirely to the zygote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FFIRequest
{
  /// Calls a function from a dynamic library with the given arguments and expected return type.
  ///
  /// `readErrno`: when true, the clone reads `errno` immediately after the C call
  /// returns and reports it back via [`FFIResponse::Ok`]'s second field. Costs one
  /// extra read when set — calls that don't need it can leave it `false`.
  Call {
    libraryPath: String,
    functionName: String,
    args: Vec<Value>,
    resultType: Type,
    readErrno: bool
  },

  /// Allocates a block of memory of the specified length in the zygote address space.
  Alloc { length: usize },
  /// Allocates enough memory to hold a dynamically-shaped struct — the byte
  /// size (with correct platform padding/alignment) is computed by `libffi`
  /// on the clone side from `fields`, not guessed by the caller. Response
  /// carries both the pointer and the resolved size (see `executeFFI`).
  AllocDynamicStruct { fields: Vec<Type> },
  /// Allocates a block of memory with specific alignment.
  /// Uses `posix_memalign` under the hood — alignment must be a power of 2.
  AllocAligned { length: usize, alignment: usize },
  /// Frees a previously allocated memory block by its pointer.
  Free { pointer: usize },

  /// Reads a raw memory block of the given length starting at the specified pointer.
  ReadMemory { pointer: usize, length: usize },
  /// Writes a value to the specified address in the zygote memory.
  WriteMemory { pointer: usize, value: Value },

  /// Reads a dynamically-typed struct at `pointer`. Field byte offsets
  /// (padding, alignment) are computed by `libffi` for the current ABI,
  /// not assumed — this is what makes [`Type::Struct`] usable for shapes
  /// that don't exist as a Rust type at compile time.
  ReadDynamicStruct { pointer: usize, fields: Vec<Type> },
  /// Writes `values` into a dynamically-typed struct at `pointer`.
  WriteDynamicStruct {
    pointer: usize,
    fields: Vec<Type>,
    values: Vec<Value>
  },

  /// Parent sends a serialized closure; the clone deserializes and stores it.
  RegisterCallback {
    id: u64,
    bytes: Vec<u8>,
    argTypes: Vec<Type>,
    returnType: Type
  },
  /// Calls a function directly by its raw memory pointer
  /// with the provided arguments and expected return type.
  ///
  /// `readErrno`: see [`FFIRequest::Call`].
  CallPointer {
    pointer: usize,
    args: Vec<Value>,
    resultType: Type,
    readErrno: bool
  },
}

/// Response to the request with the execution result or error.
#[derive(Serialize, Deserialize)]
pub enum FFIResponse
{
  /// Successful execution with the returned value, plus `errno` and — on
  /// Windows — `GetLastError`, both captured immediately after the call.
  /// `Some` only if the request asked for it via `readErrno`, `None`
  /// otherwise. The third field stays `None` outside Windows.
  Ok(Value, Option<i32>, Option<u32>),

  /// Execution failed with the corresponding error.
  Err(FFIError)
}

// =================================================================================================

/// Trait abstracting IPC between the Runtime and a Zygote Clone.
///
/// One concrete implementation per platform — see module-level docs. The
/// trait owns the whole IPC lifecycle:
/// 1. [`spawnZygote`](Transport::spawnZygote) — Runtime spawns Main Zygote and
///    gets back a control-plane handle.
/// 2. [`zygoteControlLoop`](Transport::zygoteControlLoop) — Main Zygote runs
///    its command loop forever, servicing `SpawnClone` requests.
/// 3. [`cloneEnter`](Transport::cloneEnter) — optional: a freshly cloned
///    process prepares its data endpoint and produces the
///    [`Bootstrap`](Transport::Bootstrap) that will be forwarded through the
///    control channel back to the Runtime. Backends whose clones get their
///    endpoints by inheritance (Linux) do not have this step.
/// 4. [`runtimeConnect`](Transport::runtimeConnect) — Runtime rebuilds the
///    data endpoint from the bootstrap it received.
///
/// Every step is `cfg`-clean in [`crate::zygote`] thanks to the per-platform
/// `pub type Transport = ...;` alias.
pub trait Transport: 'static
{
  /// Runtime-side data endpoint: send a request, receive a response.
  type RuntimeSide: RuntimeSide + 'static;
  /// Clone-side data endpoint: receive a request, send a response.
  type CloneSide: CloneSide + 'static;
  /// What a clone hands back through the control channel so the Runtime can
  /// reconnect to it.
  type Bootstrap: Serialize + DeserializeOwned + Send + 'static;
  /// Full handle Runtime holds for the Main Zygote (process + control plane).
  type ZygoteHandle: Send + 'static;

  /// Short backend name for diagnostics.
  #[allow(dead_code)]
  fn name() -> &'static str;

  /// Spawns the Main Zygote process and returns its handle (Runtime side).
  fn spawnZygote() -> io::Result<Self::ZygoteHandle>;

  /// Runtime → Main Zygote: asks the Main Zygote to fork / clone and return
  /// the clone's [`Bootstrap`](Transport::Bootstrap).
  fn sendSpawnClone(handle: &Self::ZygoteHandle) -> io::Result<Self::Bootstrap>;

  /// PID of a clone at the moment the Runtime receives its bootstrap.
  /// Used to populate [`crate::zygote::ClonedZygote::pid`].
  fn bootstrapPid(bootstrap: &Self::Bootstrap) -> u32;

  /// Enters the Main Zygote command loop. Called inside the freshly-spawned
  /// Main Zygote (before any clone exists). Never returns.
  ///
  /// `flag`: optional extra CLI argument used by some backends to bootstrap
  /// the control channel (e.g. an `IpcOneShotServer` name on Windows/macOS).
  fn zygoteControlLoop(flag: Option<String>) -> !;

  /// In a freshly cloned process: prepares the data endpoint and returns the
  /// bootstrap to be forwarded through the control channel.
  ///
  /// `flag`: the CLI argument the clone was launched with (matches
  /// [`zygoteControlLoop`](Transport::zygoteControlLoop)'s argument).
  ///
  /// Optional: backends whose clones inherit their endpoints from the
  /// fork have nothing to prepare and keep this default.
  #[allow(dead_code)]
  fn cloneEnter(_flag: Option<String>) -> io::Result<(Self::CloneSide, Self::Bootstrap)>
  {
    Err(io::Error::new(
      io::ErrorKind::Unsupported,
      "this backend does not enter clones: they inherit their endpoints"
    ))
  }

  /// In Runtime, after receiving [`Bootstrap`](Transport::Bootstrap) from
  /// the clone via the control channel: rebuilds the data endpoint.
  fn runtimeConnect(bootstrap: Self::Bootstrap) -> io::Result<Self::RuntimeSide>;
}

/// Runtime-side data endpoint.
pub trait RuntimeSide: Send + 'static
{
  /// Sends a serialized request. On broken pipe returns an error describing
  /// that the clone has gone away.
  fn send(&self, request: &FFIRequest) -> Result<(), String>;
  
  /// Receives a serialized response. EOF means the clone died (treated as an
  /// error here — `ClonedZygote::call` converts it into a communication-failed
  /// `FFIError`).
  fn recv(&self) -> Result<FFIResponse, String>;
}

/// Clone-side data endpoint.
pub trait CloneSide: Send + 'static
{
  /// Runs the request/response loop until the Runtime side closes the channel
  /// or a fatal error occurs (clone then `std::process::exit(0)`s — that is
  /// the isolation contract). Never returns.
  #[allow(dead_code)]
  fn run(self, cache: &mut FxHashMap<String, Library>) -> !;
}

// =================================================================================================

/// Common supertype: every backend's `ZygoteHandle` wraps the [`Child`] so the
/// supervisor loop in [`crate::zygote`] can `kill()` / `wait()` on it without
/// caring which transport sits on top.
pub struct ZygoteHandleBase
{
  /// The Main Zygote process.
  pub process: Child
}

impl Drop for ZygoteHandleBase
{
  /// Terminates the Main Zygote process.
  fn drop(&mut self) -> ()
  {
    let _ = self.process.kill();
  }
}

// =================================================================================================
