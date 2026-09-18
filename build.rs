use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
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

// =================================================================================================

/// Shared-library link flags and file extension for the current host.
///
/// - Linux / other Unix: GNU-style `-shared -fPIC` → `libfoo.so` (ELF)
/// - macOS: Apple clang `-dynamiclib` → `libfoo.dylib` (Mach-O)
///
/// Apple clang historically accepts `-shared` as a synonym for `-dynamiclib`
/// and treats `-fPIC` as a no-op, so a single command line used to work on
/// both platforms. That is an undocumented compatibility quirk, not a stable
/// contract — we branch explicitly instead.
///
/// Windows uses MSVC (`cl.exe`), a different compiler driver entirely —
/// see [`compileWindows`].
#[cfg(not(windows))]
const fn sharedLibSpec() -> (&'static [&'static str], &'static str)
{
  #[cfg(target_os = "macos")]
  {
    (&["-dynamiclib"], "dylib")
  }
  #[cfg(not(target_os = "macos"))]
  {
    (&["-shared", "-fPIC"], "so")
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

    #[cfg(windows)]
    let output: PathBuf = path.with_file_name(format!("lib{stem}.dll"));
    #[cfg(not(windows))]
    let output: PathBuf = { let (_, ext) = sharedLibSpec(); path.with_file_name(format!("lib{stem}.{ext}")) };

    if isFresh(&path, &output) { continue; }

    #[cfg(windows)]
    let compiled: bool = compileWindows(&path, &output, stem);
    #[cfg(not(windows))]
    let compiled: bool = compileUnix(&path, &output);

    if !compiled { panic!("failed to compile {}", path.display()); }
  }
}

/// Executes the system compiler to generate a shared object / dylib.
#[cfg(not(windows))]
fn compileUnix(source: &Path, output: &Path) -> bool
{
  let (flags, _) = sharedLibSpec();
  let compiler: String = env::var("CC").unwrap_or_else(|_| "cc".into());
  let status: std::io::Result<ExitStatus> = Command::new(compiler)
    .args(flags)
    .arg("-o")
    .arg(output)
    .arg(source)
    .status();

  matches!(status, Ok(s) if s.success())
}

/// Executes MSVC to generate a DLL.
///
/// `cl.exe` is located via `cc`'s registry probe rather than `PATH` — it's
/// not on `PATH` outside a Developer Command Prompt, and the probe also
/// sets up the `INCLUDE`/`LIB` environment the linker needs.
///
/// `/MD` links the shared CRT; `/MT` would give this DLL its own static CRT
/// (and its own private `errno`, invisible from this process). Import-lib
/// and object files go to `OUT_DIR`, so only the `.dll` lands next to the
/// `.c` source, matching the Unix layout.
#[cfg(windows)]
fn compileWindows(source: &Path, output: &Path, stem: &str) -> bool
{
  let target: String = env::var("TARGET").unwrap_or_default();
  let outDir: PathBuf = PathBuf::from(env::var("OUT_DIR").unwrap_or_else(|_| ".".into()));

  let mut command: Command = cc::windows_registry::find(&target, "cl.exe")
    .unwrap_or_else(|| Command::new("cl.exe"));

  let status: std::io::Result<ExitStatus> = command
    .arg("/nologo")
    .arg("/MD")
    .arg("/LD")
    .arg(format!("/Fe:{}", output.display()))
    .arg(format!("/Fo{}{}", outDir.display(), std::path::MAIN_SEPARATOR))
    .arg(source)
    .arg("/link")
    .arg(format!("/IMPLIB:{}", outDir.join(format!("{stem}.lib")).display()))
    .status();

  matches!(status, Ok(s) if s.success())
}

/// Validates if the output binary is newer than the source file.
fn isFresh(source: &Path, output: &Path) -> bool
{
  let (Ok(srcMeta), Ok(outMeta)) = (fs::metadata(source), fs::metadata(output)) else { return false };
  let (Ok(srcTime), Ok(outTime)) = (srcMeta.modified(), outMeta.modified()) else { return false };
  outTime >= srcTime
}

// =================================================================================================
