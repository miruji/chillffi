use crate::ffi::value::Pointer;
use chillffi::pathResolver::addGlobalSearchPath;
use chillffi::ffi::value::{Value};
use chillffi::call;
use chillffi::ffi;
// =================================================================================================

/// Test library resolution using direct paths, 
/// scope search paths, and global search paths
fn main() -> ()
{
  testRawPath();
  testScopePath();
  testGlobalPath();
}

/// A path with '/' — PathResolver is not involved; it goes directly to dlopen.
fn testRawPath() -> ()
{
  let result: Pointer = ffi!{
    let lib: Library = Library::load("./examples/paths/libprint.so")?;
    Ok(call!(lib, "print", Value::String(b"raw path\n".to_vec()))?)
  }.expect("raw path failed");

  assert!(matches!(result, Pointer(0)));
  println!("ok: raw path");
}

/// Temporary path through scope — resolves only inside this block.
fn testScopePath() -> ()
{
  let result: Pointer = ffi!(|scope| {
    scope.addSearchPath("examples/paths");
    let lib: Library = Library::load("libprint.so")?;
    Ok(call!(lib, "print", Value::String(b"scope path\n".to_vec()))?)
  }).expect("scope path failed");

  assert!(matches!(result, Pointer(0)));
  println!("ok: scope path");
}

/// The global path — set once, visible in all subsequent blocks.
fn testGlobalPath() -> ()
{
  addGlobalSearchPath("examples/paths");

  let result: Pointer = ffi!{
    let lib: Library = Library::load("libprint.so")?;
    Ok(call!(lib, "print", Value::String(b"global path\n".to_vec()))?)
  }.expect("global path failed");

  assert!(matches!(result, Pointer(0)));
  println!("ok: global path");
}

// =================================================================================================