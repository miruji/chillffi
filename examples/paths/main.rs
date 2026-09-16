#[path = "../platform/mod.rs"]
mod platform;
use crate::platform::platformExt;
// =================================================================================================
use chillffi::ffi;
use chillffi::ffi::types::primitive::Pointer;
use chillffi::pathResolver::addGlobalSearchPath;
// =================================================================================================

/// Library resolution: direct path, scope search path, and global search path.
fn main() -> ()
{
  rawPath();
  scopePath();
  globalPath();
}

// =================================================================================================

/// Path with '/' — goes straight to dlopen, PathResolver is skipped.
fn rawPath() -> ()
{
  let result: Pointer = ffi!(|scope| {
    let libprint: Library = scope.load(platformExt!("./examples/paths/libprint"))?;
    libprint.call("print")
      .arg("raw path\n")
      .result()
  }).expect("raw path failed");

  assert!(matches!(result, Pointer(0)));
  println!("ok: raw path");
}

/// Scope search path — only visible inside this block.
fn scopePath() -> ()
{
  let result: Pointer = ffi!(|scope| {
    scope.addSearchPath("examples/paths");
    let libprint: Library = scope.load(platformExt!("libprint"))?;
    libprint.call("print")
      .arg("scope path\n")
      .result()
  }).expect("scope path failed");

  assert!(matches!(result, Pointer(0)));
  println!("ok: scope path");
}

/// Global search path — visible in all later blocks.
fn globalPath() -> ()
{
  addGlobalSearchPath("examples/paths");

  let result: Pointer = ffi!(|scope| {
    let libprint: Library = scope.load(platformExt!("libprint"))?;
    libprint.call("print")
      .arg("global path\n")
      .result()
  }).expect("global path failed");

  assert!(matches!(result, Pointer(0)));
  println!("ok: global path");
}

// =================================================================================================
