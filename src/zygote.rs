//! Zygote process orchestration.
//!
//! Platform-neutral glue over [`ipc::Transport`]:
//! 1. spawns the Main Zygote ([`initZygote`]),
//! 2. on entry into the zygote process — enters the platform's control
//!    loop ([`runAsZygote`]),
//! 3. supervises the Main Zygote from a separate thread
//!    ([`supervisorLoop`]),
//! 4. hands out clone handles to callers via [`ClonedZygote::getMeClone`].
//!
//! All IPC details — `fork`/`RtlCloneUserProcess`, sockets / Mach ports /
//! named pipes, `SCM_RIGHTS`, `ipc-channel` — live behind the
//! [`ipc::Transport`] trait.
// =================================================================================================
pub use crate::platform::ipc::{FFIRequest, FFIResponse, ZygoteFlag};
use crate::platform::ipc::{RuntimeSide as RuntimeSideTrait, Transport as TransportTrait};
use crate::platform::low;
use parking_lot::{Mutex, MutexGuard};
use std::cell::RefCell;
use std::env;
use std::io;
use std::sync::OnceLock;
use std::thread;
// =================================================================================================

#[cfg(target_os = "linux")]
use crate::platform::ipc::linux as ipc;
#[cfg(target_os = "macos")]
use crate::platform::ipc::macos as ipc;
#[cfg(windows)]
use crate::platform::ipc::windows as ipc;

#[cfg(windows)]
pub use crate::platform::ipc::windows::runAsClone;
// =================================================================================================

/* todo
    There are several possible directions for improvements and experiments:
    1. Pair work. The idea is simple - there is 1 zygote for cloning and 2 of its clones.
       Essentially, this is a pool of cloned zygotes. So there would be 3 of them in total.
       Basically, while one is working, another one is ready to take the hit right after it.
       This should significantly reduce the load in tasks where FFIs go one after another.
    1.1.
       (By the way, for restarting the main zygote this is very important —
       you don't have to create a new dirty one, and if one died — you can quickly clone
       a duplicate through fork() for duplication).
    2. Dynamic zygote warming. The idea is also simple - depending on the load,
       we increase or decrease the number of cloned zygotes.
       This can be done using different algorithms.
    3. Splitting the Runtime into 2 parts - where the main zygote as a process initially
       will not even see the main Runtime. Something like 2 programs inside one.
       But making 2 programs is not a great approach - there needs to be a single file.
       This can be implemented in different ways. The idea is simple -
       even if the main zygote does not use Runtime instructions,
       it still declares them and they exist inside,
       although they will never be used.
       In some way, exec() and ctor solve this, but this solution fully solves it.
*/

// =================================================================================================

/// Runtime-side handle to the Main Zygote.
/// 
/// Wraps the platform-specific handle (which already owns 
/// the child process — its `Drop` calls `process.kill()`).
pub struct ZygoteHandle
{
  /// todo desc
  pub inner: ipc::ZygoteHandle
}

impl ZygoteHandle
{
  /// PID of the Main Zygote (used by the supervisor).
  pub fn pid(&self) -> u32
  {
    self.inner.base.process.id()
  }
}

/// Global state of the active zygote with synchronized access.
pub static ZygoteState: OnceLock<Mutex<ZygoteHandle>> = OnceLock::new();

// =================================================================================================

/// RAII handle for a separate zygote clone process.
pub struct ClonedZygote
{
  /// Process PID.
  pub pid: u32,

  /// Platform-specific Runtime-side data endpoint.
  pub(super) data: ipc::RuntimeSide
}

impl ClonedZygote
{
  /// Requests a clone from the main zygote and returns its RAII handle.
  pub fn getMeClone() -> io::Result<Self>
  {
    let mutex: &Mutex<ZygoteHandle> = ZygoteState.get().ok_or_else(|| {
      io::Error::new(io::ErrorKind::NotFound, "Zygote not initialized")
    })?;
    let guard: MutexGuard<ZygoteHandle> = mutex.lock();

    let bootstrap: ipc::Bootstrap =
      <ipc::Transport as TransportTrait>::sendSpawnClone(
        &guard.inner
      )
      .map_err(|e| {
        io::Error::new(
          io::ErrorKind::BrokenPipe,
          format!("SpawnClone failed: {e}")
        )
      })?;

    let pid: u32 =
      <ipc::Transport as TransportTrait>::bootstrapPid(
        &bootstrap
      );

    if pid == 0
    {
      drop(guard);
      return Err(io::Error::other(
        "Main zygote failed to create a clone (pid=0)"
      ));
    }

    let data: ipc::RuntimeSide =
      <ipc::Transport as TransportTrait>::runtimeConnect(
        bootstrap
      )?;

    Ok(Self { pid, data })
  }

  /// FFI call inside a specific clone.
  pub(super) fn call(&self, request: FFIRequest) -> Result<FFIResponse, String>
  {
    self.data.send(&request)?;
    self.data.recv()
  }
}

impl Drop for ClonedZygote
{
  /// When drop() is called, the clone is immediately killed,
  /// the main zygote is not affected.
  fn drop(&mut self) -> ()
  {
    low::killProcess(self.pid);
  }
}

// =================================================================================================

thread_local! {
  /// Zygote stack: each entry into ffi!{} puts a new zygote on top of the stack.
  pub(crate) static ZygoteStack: RefCell<Vec<ClonedZygote>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard of the active zygote in the current thread's stack.
pub struct ZygoteGuard;

impl ZygoteGuard
{
  /// Adds a zygote to the stack and returns a guard for its lifetime.
  pub fn enter(zygote: ClonedZygote) -> Self
  {
    ZygoteStack.with(|stack| {
      stack.borrow_mut().push(zygote);
    });
    Self
  }
}

impl Drop for ZygoteGuard
{
  /// Removes exactly this zygote from the stack,
  /// and Rust automatically calls its drop(), killing the process.
  fn drop(&mut self) -> ()
  {
    ZygoteStack.with(|stack| {
      stack.borrow_mut().pop();
    });
  }
}

// =================================================================================================

/// Entry point of the child Zygote process;
///
/// `main()` must call this as the first line if the first argument == [`ZygoteFlag`].
///
/// The second argument is the `IpcOneShotServer` name of the Runtime; it is
/// passed to the platform backend as `flag`.
pub fn runAsZygote() -> !
{
  // argv[2] is the bootstrap name of the control plane.
  let flag: Option<String> = env::args().nth(2);

  <ipc::Transport as TransportTrait>::zygoteControlLoop(flag)
}

/// Zygote initialization; call once,
/// as the very first line of the normal main().
pub fn initZygote() -> io::Result<()>
{
  let inner: ipc::ZygoteHandle =
    <ipc::Transport as TransportTrait>::spawnZygote()?;

  let handle: ZygoteHandle = ZygoteHandle { inner };
  ZygoteState
    .set(Mutex::new(handle))
    .map_err(|_| {
      io::Error::new(io::ErrorKind::AlreadyExists, "Zygote already initialized")
    })?;
  thread::spawn(supervisorLoop);
  Ok(())
}

// =================================================================================================

/// Supervisor: blocks on the death of the current zygote (`waitpid` /
/// `WaitForSingleObject` under the hood) and recreates it.
///
/// Separate thread — therefore [`ipc::Transport::spawnZygote`]
/// inside must go through `Command`, not `fork()`.
fn supervisorLoop() -> ()
{
  loop
  {
    let pidToWait: u32 = {
      let mutex: &Mutex<ZygoteHandle> = match ZygoteState.get()
      {
        Some(m) => m,
        None => return
      };
      mutex.lock().pid()
    };

    low::waitProcess(pidToWait);

    let mutex: &Mutex<ZygoteHandle> = ZygoteState.get().unwrap();
    let mut guard: MutexGuard<ZygoteHandle> = mutex.lock();
    if guard.pid() == pidToWait // Not recreated in parallel yet through call()
    {
      match initZygoteInner()
      {
        Ok(newHandle) =>
        {
          *guard = newHandle;
        }
        Err(_) =>
        {
          drop(guard);
          thread::sleep(std::time::Duration::from_millis(200));
        }
      }
    }
  }
}

/// Re-spawns the Main Zygote without re-initializing the [`ZygoteState`]
/// (used by the supervisor after a crash).
fn initZygoteInner() -> io::Result<ZygoteHandle>
{
  let inner: ipc::ZygoteHandle =
    <ipc::Transport as TransportTrait>::spawnZygote()?;
  Ok(ZygoteHandle { inner })
}

// =================================================================================================

#[cfg(test)]
mod tests
{
  use crate::ffi;
  use crate::ffi::errors::FFIError;
  use crate::platform::{platformExt, LibmPath};
  // ===============================================================================================

  /// The headline guarantee of this crate: a real SIGSEGV inside an
  /// isolated clone must surface as `Err`, never take down the process
  /// running this test. Uses `examples/isolation/crash.c`, built by
  /// `build.rs` for every `cargo build`/`cargo test`, not just `cargo run
  /// --example isolation`.
  #[test]
  fn segfaultIsIsolated() -> ()
  {
    let result: Result<(), FFIError> = ffi!(|scope| {
      scope.addSearchPath("examples/isolation");
      let lib = scope.load(platformExt!("libcrash"))?;
      lib.call("triggerSegfault").void()
    });

    let err: FFIError =
      result.expect_err("a segfaulting clone must not report success");
    assert!(
      matches!(err, FFIError::ZygoteCommunicationFailed(_)),
      "unexpected error: {err:?}"
    );
  }

  /// Same guarantee for `abort()` (SIGABRT) — a different signal, same boundary.
  #[test]
  fn abortIsIsolated() -> ()
  {
    let result: Result<(), FFIError> = ffi!(|scope| {
      scope.addSearchPath("examples/isolation");
      let lib = scope.load(platformExt!("libcrash"))?;
      lib.call("triggerAbort").void()
    });

    let err: FFIError =
      result.expect_err("an aborting clone must not report success");
    assert!(
      matches!(err, FFIError::ZygoteCommunicationFailed(_)),
      "unexpected error: {err:?}"
    );
  }

  /// A crashed clone must not affect the main zygote: a completely
  /// unrelated `ffi!` block right after still succeeds. Each `ffi!` forks
  /// its own fresh clone from the always-alive main zygote — a crashed
  /// clone never touches the main zygote itself, so this needs no
  /// supervisor-restart delay to be meaningful.
  #[test]
  fn runtimeSurvivesAfterCrash() -> ()
  {
    let crashed: Result<(), FFIError> = ffi!(|scope| {
      scope.addSearchPath("examples/isolation");
      let lib = scope.load(platformExt!("libcrash"))?;
      lib.call("triggerAbort").void()
    });
    assert!(crashed.is_err(), "sanity check: the setup call should have crashed");

    let result: f64 = ffi!(|scope| {
      let libm = scope.load(LibmPath)?;
      libm.call("sqrt").arg::<f64>(16.0).result()
    })
    .expect("runtime should survive a crashed clone");

    assert!((result - 4.0).abs() < f64::EPSILON);
  }

  // ===============================================================================================

  /// Repeated sequential `ffi!` blocks stress the control plane:
  /// each iteration does `SpawnClone` → IPC bootstrap → drop/kill.
  /// This is the minimal regression for the IPC race that previously
  /// appeared only under high test volume (many independent clones
  /// from one main zygote in a single process).
  #[test]
  fn sequentialCloneStress() -> ()
  {
    const Iterations: usize = 50;

    for i in 0..Iterations
    {
      let result: f64 = ffi!(|scope| {
        let libm = scope.load(LibmPath)?;
        libm.call("sqrt").arg::<f64>(4.0).result()
      })
      .unwrap_or_else(|e| {
        panic!("sequential clone stress failed on iteration {i}: {e}")
      });

      assert!(
        (result - 2.0).abs() < f64::EPSILON,
        "unexpected sqrt result on iteration {i}"
      );
    }
  }

  /// Concurrent `ffi!` from several threads is the scenario that
  /// exposed the original Darwin IPC bug (dead FD / "Bogus destination
  /// port" under parallel `SpawnClone`). Channels are created after
  /// `fork` and handed over via `IpcOneShotServer`; this test keeps
  /// pressure on that path so a regression cannot hide behind
  /// sequential-only runs.
  #[test]
  fn concurrentCloneStress() -> ()
  {
    use std::thread;

    const Threads: usize = 8;
    const PerThread: usize = 20;

    let handles: Vec<thread::JoinHandle<()>> = (0..Threads)
      .map(|t| {
        thread::spawn(move || {
          for i in 0..PerThread
          {
            let result: f64 = ffi!(|scope| {
              let libm = scope.load(LibmPath)?;
              libm.call("sqrt").arg::<f64>(4.0).result()
            })
            .unwrap_or_else(|e| {
              panic!("concurrent clone stress failed on thread {t} iteration {i}: {e}")
            });

            assert!(
              (result - 2.0).abs() < f64::EPSILON,
              "unexpected sqrt result on thread {t} iteration {i}"
            );
          }
        })
      })
      .collect();

    for handle in handles
    {
      handle.join().expect("concurrent clone stress thread panicked");
    }
  }

  /// Pure create/drop of clones without any FFI call.
  /// Isolates the control-plane path (`SpawnClone` + channel
  /// bootstrap + kill) from library loading and `libffi` work.
  #[test]
  fn rapidCloneCreateDrop() -> ()
  {
    const Iterations: usize = 100;

    for i in 0..Iterations
    {
      let zygote: crate::zygote::ClonedZygote = crate::zygote::ClonedZygote::getMeClone()
        .unwrap_or_else(|e| panic!("getMeClone failed on iteration {i}: {e}"));
      drop(zygote);
    }
  }

  // ===============================================================================================
}
