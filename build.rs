use std::process::ExitStatus;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
// =================================================================================================

fn main() -> ()
{
  // Compiles C sources within the examples directory.
  let examplesDir: &Path = Path::new("examples");
  if examplesDir.exists() 
  { // Watching each individual .c file only protects files Cargo
    // already knew about the last time this ran — a brand new .c file was
    // never in that list, so it stayed invisible and never got compiled.
    // Watching the directory itself covers additions too.
    println!("cargo:rerun-if-changed=examples");
    compileDir(examplesDir);
  }
}

/// Recursively compiles C source files into shared libraries.
fn compileDir(dir: &Path) -> ()
{
  let Ok(entries) = fs::read_dir(dir) else { return };

  for entry in entries.flatten()
  {
    let path: PathBuf = entry.path();

    // Traverse subdirectories.
    if path.is_dir() { compileDir(&path); continue; }
    if path.extension().and_then(|e| e.to_str()) != Some("c") { continue; }

    // Notify Cargo to rebuild on source modification.
    println!("cargo:rerun-if-changed={}", path.display());

    let stem: &str = path.file_stem().unwrap().to_str().unwrap();
    let output: PathBuf = path.with_file_name(format!("lib{}.so", stem));

    if isFresh(&path, &output) { continue; }

    // Execute compiler to generate a shared object.
    let compiler: String = env::var("CC").unwrap_or_else(|_| "cc".into());
    let status: std::io::Result<ExitStatus> = Command::new(compiler)
      .args(["-shared", "-fPIC", "-o"])
      .arg(&output)
      .arg(&path)
      .status();

    match status {
      Ok(s) if s.success() => {}
      _ => panic!("failed to compile {}", path.display()),
    }
  }
}

/// Validates if the output binary is newer than the source file.
fn isFresh(source: &Path, output: &Path) -> bool
{
  let (Ok(srcMeta), Ok(outMeta)) = (fs::metadata(source), fs::metadata(output)) else { return false };
  let (Ok(srcTime), Ok(outTime)) = (srcMeta.modified(), outMeta.modified()) else { return false };
  outTime >= srcTime
}

// =================================================================================================