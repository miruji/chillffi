use std::sync::MutexGuard;
use crate::zygote::ClonedZygote;
use crate::zygote::ZygoteState;
use crate::ffi::value::{Type, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use crate::zygote::{FFIRequest, FFIResponse, ZygoteStack};
// =================================================================================================

/// Ошибки, возникающие при работе с загрузкой библиотек и выполнением вызовов
#[derive(Debug)]
pub enum FFIError
{
  ZygoteNotInitialized,
  NoActiveZygoteScope,
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

impl<E: std::error::Error + 'static> From<E> for FFIError
{
  fn from(err: E) -> Self {
    Self::Other(err.to_string())
  }
}

// =================================================================================================

/// Счётчик для выдачи уникальных идентификаторов библиотекам
static NextLibraryID: AtomicUsize = AtomicUsize::new(1);
/// Глобальный реестр загруженных библиотек по их идентификаторам
static RegisteredLibraries: OnceLock<Mutex<HashMap<usize, String>>> = OnceLock::new();

/// Возвращает следующий уникальный идентификатор библиотеки
fn nextLibraryId() -> usize
{
  NextLibraryID.fetch_add(1, Ordering::SeqCst)
}

/// Возвращает общий реестр зарегистрированных библиотек
fn getRegistry() -> &'static Mutex<HashMap<usize, String>>
{
  RegisteredLibraries.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Добавляет библиотеку в реестр по её идентификатору
fn registerLibrary(id: usize, path: &str) -> ()
{
  let mut registry = getRegistry().lock().unwrap();
  registry.insert(id, path.to_string());
}

/// Удаляет библиотеку из реестра по её идентификатору
fn unregisterLibrary(id: usize) -> ()
{
  let mut registry = getRegistry().lock().unwrap();
  registry.remove(&id);
}

/// Запрашивает загрузку библиотеки в текущую зиготу
fn sendLoadLibrary(_id: usize, _path: &str) -> Result<(), FFIError>
{
  // todo: Отправить ZygoteCommand::LoadLibrary в зиготу для кеширования
  //   Пока что библиотека будет загружаться при каждом вызове через callById
  //   - только резервирует место для будущего кеширования библиотек.
  Ok(())
}

/// Запрашивает выгрузку библиотеки из текущей зиготы
fn sendUnloadLibrary(_id: usize) -> Result<(), FFIError>
{
  // todo: Отправить ZygoteCommand::UnloadLibrary в зиготу
  //   Пока операция является заглушкой до реализации кеширования.
  Ok(())
}

/// Выполняет вызов FFI функции по идентификатору зарегистрированной библиотеки
fn callById(
  libraryId: usize,
  functionName: &str,
  args: Vec<Value>,
  resultType: Type,
) -> Result<Value, FFIError> 
{
  // Проверяем, инициализирована ли глобальная зигота в ZygoteState
  if ZygoteState.get().is_none() {
    return Err(FFIError::ZygoteNotInitialized);
  }

  // Достаем путь к `.so`/`.dll` из реестра и формируем FFIRequest
  let registry: MutexGuard<HashMap<usize, String>> = getRegistry().lock().unwrap();
  let libraryPath: String = registry
    .get(&libraryId)
    .ok_or(FFIError::LibraryNotFound)?
    .clone();
  drop(registry);

  let request: FFIRequest = FFIRequest {
    libraryPath,
    functionName: functionName.to_string(),
    args,
    resultType,
  };
  
  // Ищем активный клон в локальном стеке текущего потока
  ZygoteStack.with(|stack| {
    let mut mutStack = stack.borrow_mut();

    // Если стек пуст — значит вызов идет вне контекста ffi!{}
    let zygote: &mut ClonedZygote = mutStack
      .last_mut()
      .ok_or(FFIError::NoActiveZygoteScope)?;

    // Выполняем FFI запрос через текущую зиготу
    match zygote.call(request) {
      Ok(FFIResponse::Ok(val)) => Ok(val),
      Ok(FFIResponse::Err(err)) => Err(FFIError::CallFailed(err)),
      Err(err) => Err(FFIError::ZygoteCommunicationFailed(err)),
    }
  })
}

// =================================================================================================

/// Дескриптор загруженной библиотеки с ограничением доступных методов
#[doc(hidden)]
pub struct __Library<const Allowed: bool = false>
{
  // Идентификатор библиотеки внутри менеджера
  libraryId: usize,
  // Путь к загруженной библиотеке
  libraryPath: String
}

/// Методы, доступные всегда (id, и т.д.)
impl<const Allowed: bool> __Library<Allowed>
{
  /// Возвращает идентификатор библиотеки
  pub fn id(&self) -> usize
  {
    self.libraryId
  }
}

// Методы, доступные только внутри ffi!{}
impl __Library<true>
{
  /// Выполняет вызов функции из загруженной библиотеки
  pub fn call(
    &self,
    functionName: &str,
    args: Vec<Value>,
    resultType: Type,
  ) -> Result<Value, FFIError>
  {
    callById(self.libraryId, functionName, args, resultType)
  }

  /// Выгружает библиотеку и удаляет её из реестра
  pub fn unload(self) -> Result<(), FFIError>
  {
    sendUnloadLibrary(self.libraryId)?;
    unregisterLibrary(self.libraryId);
    Ok(())
  }

  /// Загружает библиотеку и регистрирует её для дальнейших вызовов
  pub fn load(libraryPath: &str) -> Result<Self, FFIError>
  {
    let libraryId: usize = nextLibraryId();
    let ownedPath: String = String::from(libraryPath);
    registerLibrary(libraryId, &ownedPath);
    match sendLoadLibrary(libraryId, &ownedPath)
    {
      Ok(()) => Ok(Self{ libraryId, libraryPath: ownedPath }),
      Err(error) => { unregisterLibrary(libraryId); Err(error) }
    }
  }
}

/// Публичный тип снаружи — без load/call
pub type Library = __Library<false>;

/// Скрытый тип для ffi! — с load/call
#[doc(hidden)]
pub type __FFILibrary = __Library<true>;

// =================================================================================================