use chillffi::pathResolver::addGlobalSearchPath;
use crate::ffi::types::primitive::Pointer;
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
  let result: Pointer = ffi!(|scope| {
    let libprint: Library = scope.load("./examples/paths/libprint.so")?;
    Ok(
      libprint.call("print")
        .arg("raw path\n")
        .result()?
    )
  }).expect("raw path failed");

  assert!(matches!(result, Pointer(0)));
  println!("ok: raw path");
}

/// Temporary path through scope — resolves only inside this block.
fn testScopePath() -> ()
{
  let result: Pointer = ffi!(|scope| {
    scope.addSearchPath("examples/paths");
    let libprint: Library = scope.load("libprint.so")?;
    Ok(
      libprint.call("print")
        .arg("scope path\n")
        .result()?
    )
  }).expect("scope path failed");

  assert!(matches!(result, Pointer(0)));
  println!("ok: scope path");
}

/// The global path — set once, visible in all subsequent blocks.
fn testGlobalPath() -> ()
{
  addGlobalSearchPath("examples/paths");

  let result: Pointer = ffi!(|scope| {
    let libprint: Library = scope.load("libprint.so")?;
    Ok(
      libprint.call("print")
        .arg("global path\n")
        .result()?
    )
  }).expect("global path failed");

  assert!(matches!(result, Pointer(0)));
  println!("ok: global path");
}

// =================================================================================================