//! A path resolver for searching for files. Contains a global instance, but
//! a declaration inside `scope` can also be used for local lists.
// =================================================================================================
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::OnceLock;
// =================================================================================================

/// A resolver that searches for files in a list of directories.
#[derive(Default)]
pub struct PathResolver
{
  /// List of directories to search for files.
  dirs: Vec<PathBuf>
}

impl PathResolver
{
  /// Adds a directory to the search path.
  pub fn addPath(&mut self, path: impl Into<PathBuf>) -> ()
  {
    self.dirs.push(path.into());
  }

  /// Resolves a file name by searching in the registered directories. 
  /// 
  /// Returns None if the name contains a slash or the file is not found.
  pub fn resolve(&self, name: &str) -> Option<String>
  {
    if name.contains('/') { return None; }
    self.dirs.iter()
      .map(|dir| dir.join(name))
      .find(|p| p.exists())
      .map(|p| p.to_string_lossy().into_owned())
  }
}

// =================================================================================================

/// Global singleton holding the shared path resolver.
static GlobalPaths: OnceLock<RwLock<PathResolver>> = OnceLock::new();

/// Adds a directory to the global search path, initializing the resolver if not already present.
pub fn addGlobalSearchPath(path: impl Into<PathBuf>) -> ()
{
  GlobalPaths.get_or_init(|| RwLock::new(PathResolver::default()))
    .write()
    .addPath(path);
}

/// Resolves a file name using the global search path. 
/// 
/// Returns None if the global resolver is not initialized or the file is not found.
pub(super) fn resolveGlobal(name: &str) -> Option<String>
{
  GlobalPaths.get()?
    .read()
    .resolve(name)
}

// =================================================================================================

#[cfg(test)]
mod tests
{
  use super::*;
  use std::fs::File;
  use std::env::temp_dir;
  // ===============================================================================================

  /// Checks that paths containing a slash are ignored.
  #[test]
  fn ignoresPathWithSlash() -> ()
  {
    let resolver: PathResolver = PathResolver::default();
    assert_eq!(resolver.resolve("foo/bar.so"), None);
  }

  /// Checks that None is returned when the file is not found.
  #[test]
  fn returnsNoneWhenNotFound() -> ()
  {
    let mut resolver: PathResolver = PathResolver::default();
    resolver.addPath("/nonexistent/dir");
    assert_eq!(resolver.resolve("libNope.so"), None);
  }

  /// Checks finding an existing file in the registered directories.
  #[test]
  fn findsExistingFile() -> ()
  {
    let dir: PathBuf = temp_dir();
    let fileName: &str = "chillffiTestResolve.so";
    File::create(dir.join(fileName)).unwrap();

    let mut resolver: PathResolver = PathResolver::default();
    resolver.addPath(&dir);

    let resolved: String = resolver.resolve(fileName).expect("should find file");
    assert!(resolved.ends_with(fileName));

    std::fs::remove_file(dir.join(fileName)).unwrap();
  }

  /// Checks resolving a file name via the global search path.
  #[test]
  fn globalRoundtrip() -> ()
  {
    let dir: PathBuf = temp_dir();
    let fileName: &str = "chillffiTestGlobal.so";
    File::create(dir.join(fileName)).unwrap();

    addGlobalSearchPath(&dir);
    let resolved: String = resolveGlobal(fileName).expect("should find via global");
    assert!(resolved.ends_with(fileName));

    std::fs::remove_file(dir.join(fileName)).unwrap();
  }

  // ===============================================================================================
}

// =================================================================================================