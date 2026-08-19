use crate::ffi::value::{Type, Value};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use crate::zygote::{FFIRequest, FFIResponse, ZygoteStack};
// =================================================================================================

/// todo desc
#[derive(Debug)]
pub enum FFIError
{
  ZygoteNotInitialized,
  ZygoteCommunicationFailed(String),
  LibraryLoadFailed(String),
  LibraryNotFound,
  SymbolNotFound,
  BadArgument,
  BadResultType,
  CallFailed(String),
  UnsupportedPointerReturn,
  EncodeFailed,
  DecodeFailed,
  Other(String)
}

impl std::fmt::Display for FFIError
{
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
  {
    write!(f, "{:?}", self)
  }
}

impl std::error::Error for FFIError {}

impl From<String> for FFIError {
  fn from(s: String) -> Self {
    FFIError::Other(s)
  }
}

// =================================================================================================

/// todo desc
static NextLibraryID: AtomicU64 = AtomicU64::new(1);
/// todo desc
static RegisteredLibraries: OnceLock<Mutex<HashMap<u64, String>>> = OnceLock::new();

/// todo desc
fn nextLibraryId() -> u64
{
  NextLibraryID.fetch_add(1, Ordering::SeqCst)
}

/// todo desc
fn getRegistry() -> &'static Mutex<HashMap<u64, String>>
{
  RegisteredLibraries.get_or_init(|| Mutex::new(HashMap::new()))
}

/// todo desc
fn registerLibrary(id: u64, path: &str)
{
  let mut registry = getRegistry().lock().unwrap();
  registry.insert(id, path.to_string());
}

/// todo desc
fn unregisterLibrary(id: u64)
{
  let mut registry = getRegistry().lock().unwrap();
  registry.remove(&id);
}

/// todo desc
fn sendLoadLibrary(_id: u64, _path: &str) -> Result<(), FFIError>
{
  // todo: Отправить ZygoteCommand::LoadLibrary в зиготу для кеширования
  // Пока что библиотека будет загружаться при каждом вызове через callById
  Ok(())
}

/// todo desc
fn sendUnloadLibrary(_id: u64) -> Result<(), FFIError>
{
  // todo: Отправить ZygoteCommand::UnloadLibrary в зиготу
  Ok(())
}

/// todo desc
fn callById(
  libraryId: u64,
  functionName: &str,
  args: Vec<Value>,
  resultType: Type,
) -> Result<Value, FFIError> 
{
  let registry: MutexGuard<HashMap<u64, String>> = getRegistry().lock().unwrap();
  let libraryPath: String = registry.get(&libraryId)
    .ok_or(FFIError::LibraryNotFound)?
    .clone();
  drop(registry);

  let request: FFIRequest = FFIRequest {
    libraryPath,
    functionName: functionName.to_string(),
    args,
    resultType,
  };

  // Выбираем клон Зиготы, созданный в текущем блоке ffi!{}
  ZygoteStack.with(|stack| {
    let mut mut_stack = stack.borrow_mut();
    let zygote = mut_stack.last_mut().ok_or(FFIError::ZygoteNotInitialized)?;

    match zygote.call(request) {
      Ok(FFIResponse::Ok(value)) => Ok(value),
      Ok(FFIResponse::Err(e)) => Err(FFIError::CallFailed(e)),
      Err(e) => Err(FFIError::ZygoteCommunicationFailed(e)),
    }
  })
}

// =================================================================================================

#[doc(hidden)]
pub struct __Library<const Allowed: bool = false>
{
  libraryId: u64,
  libraryPath: String
}

// методы, доступные ВСЕГДА (id, и т.д.)
impl<const Allowed: bool> __Library<Allowed>
{
  pub fn id(&self) -> u64
  {
    self.libraryId
  }
}

// методы, доступные ТОЛЬКО внутри ffi!{}
impl __Library<true>
{
  pub fn call(
    &self,
    functionName: &str,
    args: Vec<Value>,
    resultType: Type,
  ) -> Result<Value, FFIError>
  {
    callById(self.libraryId, functionName, args, resultType)
  }

  pub fn unload(self) -> Result<(), FFIError>
  {
    sendUnloadLibrary(self.libraryId)?;
    unregisterLibrary(self.libraryId);
    Ok(())
  }

  pub fn load(libraryPath: &str) -> Result<__Library<true>, FFIError>
  {
    let libraryId: u64 = nextLibraryId();
    let ownedPath: String = String::from(libraryPath);
    registerLibrary(libraryId, &ownedPath);
    match sendLoadLibrary(libraryId, &ownedPath)
    {
      Ok(()) => Ok(__Library { libraryId, libraryPath: ownedPath }),
      Err(error) => { unregisterLibrary(libraryId); Err(error) }
    }
  }
}

// публичный тип снаружи — без load/call
pub type Library = __Library<false>;

// скрытый тип для ffi! — с load/call
#[doc(hidden)]
pub type __FFILibrary = __Library<true>;

// =================================================================================================

/// todo desc
#[derive(Serialize, Deserialize)]
pub enum CallTarget
{
  Path(String),
  LibraryId(u64)
}

/// todo desc
#[derive(Serialize, Deserialize)]
pub struct CallRequest
{
  pub target: CallTarget,
  pub functionName: String,
  pub args: Vec<Value>,
  pub resultType: Type
}

/// todo desc
#[derive(Serialize, Deserialize)]
pub enum ZygoteCommand
{
  LoadLibrary(LoadLibraryRequest),
  UnloadLibrary(UnloadLibraryRequest),
  Call(CallRequest)
}

/// todo desc
#[derive(Serialize, Deserialize)]
pub struct LoadLibraryRequest
{
  pub libraryId: u64,
  pub libraryPath: String
}

/// todo desc
#[derive(Serialize, Deserialize)]
pub struct UnloadLibraryRequest
{
  pub libraryId: u64
}

// =================================================================================================