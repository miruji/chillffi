use std::cell::RefCell;
use std::env;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixStream};
use std::os::fd::{AsRawFd, FromRawFd, RawFd, OwnedFd};
use std::path::PathBuf;
use std::process::{Command, Child, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use bincode::config::Configuration;
use fxhash::FxHashMap;
use libloading::Library;
use serde::{Serialize, Deserialize};
use crate::ffi::value::{Type, Value};
use crate::worker::executeFFI;
// =================================================================================================

/* todo
    Есть хорошая статья https://kobzol.github.io/rust/2024/01/28/process-spawning-performance-in-rust.html
    Можно попробовать сделать что-то из этого, для улучшения работы:
    | Сценарий             | Решение                                |
    | -------------------- | -------------------------------------- |
    | десятки процессов    | `Command` нормально                    |
    | тысячи процессов/сек | проверять glibc/kernel                 |
    | большой RSS          | избегать fork                          |
    | HPC                  | использовать worker pool               |
    | много env            | минимизировать environment             |
    | Rust async           | `spawn_blocking` или отдельные workers |
    Возможно есть более лучшие способы.

   todo
    Кроме того есть еще несколько направлений:
    1. Парная работа. Идея простая - есть 1 зигота для клонирования и 2 её клона.
       Собственно пока один работает - другая готова принять удар следом за ней. 
       Это должно хорошо снижать нагрузку в задачах, когда FFI идут друг за другом.
    2. Динамический прогрев зигот. Идея тоже простая - в зависимости от нагрузки 
       мы добавляем или уменьшаем количество процессов.
       Это можно сделать разными алгоритмами. 
       Это уже не обязательная и экспериментальная область.
    3. Разделение Runtime на 2 части - где зигота как процесс изначально даже 
       не будет видеть основной Runtime. Что-то вроде 2 программы в одной. 
       Но я не хочу делать 2 программы - чтобы файл был один. 
       Это можно реализовать разными способами. Идея простая - 
       даже если зигота не использует инструкции Runtime - 
       то она все равно объявляет их и они существуют внутри, 
       хотя никогда не будут использованы. Это тоже экспериментальное направление.
*/

// =================================================================================================

/// Скрытый флаг запуска: если он первый аргумент — это не runtime, а процесс-Зигота.
pub const ZygoteFlag: &str = "__zygote";

/// Запрос на выполнение FFI, уходящий в Зиготу целиком
#[derive(Serialize, Deserialize)]
pub struct FFIRequest
{
  /// Путь к библиотеке, в которой находится вызываемая функция
  pub libraryPath: String,
  /// Имя функции для вызова в загруженной библиотеке
  pub functionName: String,
  /// Аргументы, передаваемые в функцию
  pub args: Vec<Value>,
  /// Ожидаемый тип возвращаемого значения
  pub resultType: Type
}

/// Ответ на запрос с результатом выполнения или ошибкой
#[derive(Serialize, Deserialize)]
pub enum FFIResponse
{
  Ok(Value),
  Err(String)
}

/// Управляет процессом зиготы и каналом связи с ним
pub(super) struct ZygoteHandle
{
  /// Дочерний процесс зиготы
  process: Child,
  /// Сокет для обмена запросами и ответами с процессом
  pub(super) socket: UnixStream
}

impl Drop for ZygoteHandle
{
  /// Завершает процесс зиготы и удаляет временный сокет
  fn drop(&mut self) -> ()
  {
    let _ = self.process.kill();
  }
}

/// Глобальное состояние активной зиготы с синхронизацией доступа
pub(super) static ZygoteState: OnceLock<Mutex<ZygoteHandle>> = OnceLock::new();

// =================================================================================================

/// RAII-хэндл для отдельного процесса-клона зиготы
pub struct ClonedZygote 
{
  /// PID процесса
  pub pid: libc::pid_t,
  /// Сокет для обмена запросами
  pub socket: UnixStream,
}

impl ClonedZygote 
{
  /// Запрашивает клон у Главной Зиготы и возвращает его RAII-хэндл
  pub fn getMeClone() -> io::Result<Self>
  {
    let mutex: &Mutex<ZygoteHandle> = ZygoteState.get().expect("Zygote not initialized");
    let mut guard: MutexGuard<ZygoteHandle> = mutex.lock().unwrap();

    // Отправляем сигнал на создание клона
    writeMessage(&mut guard.socket, &[1u8])?;

    // Получаем PID клона
    let pidBytes: Vec<u8> = readMessage(&mut guard.socket)?;
    let pid: i32 = i32::from_le_bytes(pidBytes.try_into().unwrap());

    // Читаем дескриптор сокета напрямую из RAM
    let fd: RawFd = recvFd(&mut guard.socket)?;
    let socket: UnixStream = unsafe { UnixStream::from_raw_fd(fd) };

    drop(guard);
    Ok(Self { pid, socket })
  }

  /// Вызов FFI внутри конкретного клона
  pub fn call(&mut self, request: FFIRequest) -> Result<FFIResponse, String>
  {
    let bytes: Vec<u8> = encode(&request);
    let responseBytes: Vec<u8> = sendAndReceive(&mut self.socket, &bytes).map_err(|e| e.to_string())?;
    decode(&responseBytes).map_err(|e| e.to_string())
  }
}

impl Drop for ClonedZygote 
{
  /// При drop() клон моментально убивается, Главная Зигота не затрагивается
  fn drop(&mut self) -> ()
  {
    unsafe {
      libc::kill(self.pid, libc::SIGKILL);
    }
  }
}

// =================================================================================================

thread_local!{
  /// Стек зигот: каждый вход в ffi!{} кладет новую зиготу наверх стека
  pub(crate) static ZygoteStack: RefCell<Vec<ClonedZygote>> = const { RefCell::new(Vec::new()) };
}

/// RAII-охранник активной зиготы в стеке текущего потока.
pub struct ZygoteGuard;

impl ZygoteGuard 
{
  /// Добавляет зиготу в стек и возвращает охранник её времени жизни.
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
  /// Снимаем со стека именно эту зиготу,
  /// и Rust автоматически вызывает её drop(), убивая процесс.
  fn drop(&mut self) -> ()
  {
    ZygoteStack.with(|stack| {
      stack.borrow_mut().pop();
    });
  }
}

// =================================================================================================

/// Точка входа дочернего процесса-Зиготы;
/// main() обязан вызвать это первой строкой, если первый аргумент == ZygoteFlag;
/// Процесс порождён через Command (fork+exec) — runtime не прогревался,
/// Лишних задач нет, кучи метаданных нет. Библиотеку заранее НЕ грузит.
pub (super) fn runAsZygote() -> !
{
  // Забираем сокет прямо из STDIN
  let socket: UnixStream = unsafe { UnixStream::from_raw_fd(libc::STDIN_FILENO) };
  zygoteLoop(socket);
}

/// Инициализация Зиготы; вызывать один раз, самой первой строкой обычного main(),
/// до args-парсинга, до чтения файла, до parseLines/readTokens.
pub(super) fn initZygote() -> io::Result<()>
{
  let handle: ZygoteHandle = spawnZygote()?;
  ZygoteState.set(Mutex::new(handle))
    .map_err(|_| io::Error::new(io::ErrorKind::AlreadyExists, "Zygote already initialized"))?;
  thread::spawn(supervisorLoop);
  Ok(())
}

/// Зигота порождается ТОЛЬКО через Command (fork+exec) — и при старте, и при пересоздании.
/// Это принципиально: обычный fork() пересоздания из уже прогретого многопоточного runtime
/// (супервизор — отдельный поток) унаследовал бы чужие мьютексы в захваченном состоянии —
/// та самая дедлок-ловушка из твоего разбора. exec() полностью заменяет образ процесса,
/// поэтому Зигота всегда рождается чистой, независимо от того, насколько "толстым"
/// успел стать runtime к моменту respawn'а.
pub(super) fn spawnZygote() -> io::Result<ZygoteHandle>
{
  // Создаем парный сокет прямо в RAM без участия файловой системы
  let (runtimeSocket, zygoteSocket): (UnixStream, UnixStream) = UnixStream::pair()?;
  
  //
  let currentExe: PathBuf = env::current_exe()?; // todo Может failed, если путь к исполняемому файлу слишком длинный или нет прав?
  let process: Child = Command::new(currentExe)
    .arg(ZygoteFlag)
    .stdin(Stdio::from(OwnedFd::from(zygoteSocket)))
    .spawn()?;

  Ok(ZygoteHandle{ process, socket: runtimeSocket })
}

/// Цикл главной зиготы: бесконечный цикл ожидания команд.
/// Библиотеку заранее НЕ грузит — язык интерпретируемый, какой FFI понадобится, неизвестно
/// заранее. Зигота — пустой рантайм-шаблон; dlopen делает только форкнутый зигота.
fn zygoteLoop(mut socket: UnixStream) -> !
{
  // Важно: Игнорирование SIGCHLD нужно только в главной Зиготе.
  // Это заставляет ядро ОС автоматически очищать её клоны при завершении (без зомби).
  // В main runtime это писать нельзя: там waitpid в supervisorLoop() отслеживает сам процесс Зиготы,
  // и при SIG_IGN он упадет с ECHILD и уйдет в гарантированную нагрузку CPU.
  unsafe { libc::signal(libc::SIGCHLD, libc::SIG_IGN); }

  //
  loop 
  {
    // Получаем путь к сокету для нового клона
    if readMessage(&mut socket).is_err() {
      std::process::exit(0); // Parent умер
    }

    // Создаем парный сокет в памяти для нового клона
    let (runtimeSocket, cloneSocket): (UnixStream, UnixStream) = match UnixStream::pair() {
      Ok(pair) => pair,
      Err(_) => {
        let _ = writeMessage(&mut socket, &0i32.to_le_bytes());
        continue;
      }
    };

    match unsafe { libc::fork() } 
    {
      -1 => {
        let _ = writeMessage(&mut socket, &0i32.to_le_bytes());
      }
      0 => {
        // Клон-зигота: закрываем родительскую сторону и уходим в цикл
        drop(runtimeSocket);
        cloneLoop(cloneSocket);
      }
      pid => {
        // Главная зигота: отправляем PID и пересылаем дескриптор сокета в Runtime
        drop(cloneSocket);
        if writeMessage(&mut socket, &pid.to_le_bytes()).is_ok() {
          let _ = sendFd(&mut socket, runtimeSocket.as_raw_fd());
        }
      }
    }
    //
  }
}

/// Персональный цикл клона
fn cloneLoop(mut socket: UnixStream) -> ! 
{
  let mut libraryCache: FxHashMap<String, Library> = FxHashMap::default();
  
  loop 
  {
    let requestBytes: Vec<u8> = match readMessage(&mut socket) 
    {
      Ok(bytes) => bytes,
      Err(_) => std::process::exit(0), // Переменную drop'нули — сокет закрылся, клон ушел
    };

    let response: FFIResponse = handleRequest(&requestBytes, &mut libraryCache);
    if writeMessage(&mut socket, &encode(&response)).is_err() {
      std::process::exit(0);
    }
  }
}

/// Обрабатывает входящий запрос и выполняет FFI операцию с использованием кеша библиотек.
/// Возвращает результат выполнения или описание ошибки.
fn handleRequest(requestBytes: &[u8], cache: &mut FxHashMap<String, Library>) -> FFIResponse
{
  match decode::<FFIRequest>(requestBytes)
  {
    Ok(request) => match executeFFI(request, cache)
    {
      Ok(value) => FFIResponse::Ok(value),
      Err(e)    => FFIResponse::Err(e),
    },
    Err(e) => FFIResponse::Err(format!("Bad request: {}", e)),
  }
}

// =================================================================================================

/// Вызывается из workerManager::callExternal;
/// при обрыве связи с Зиготой — пересоздаёт её (через Command) и повторяет запрос один раз.
pub fn call(request: FFIRequest) -> Result<FFIResponse, String>
{
  let bytes: Vec<u8> = encode(&request);
  let mutex: &Mutex<ZygoteHandle> = ZygoteState.get().expect("Zygote not initialized");
  let mut guard: MutexGuard<ZygoteHandle> = mutex.lock().unwrap();

  if let Ok(responseBytes) = sendAndReceive(&mut guard.socket, &bytes)
  {
    return decode(&responseBytes).map_err(|e| e.to_string());
  }

  *guard = spawnZygote().map_err(|e| format!("Zygote respawn failed: {}", e))?;
  let responseBytes: Vec<u8> = sendAndReceive(&mut guard.socket, &bytes).map_err(|e| e.to_string())?;
  drop(guard);
  
  decode(&responseBytes).map_err(|e| e.to_string())
}

/// Отправляет сообщение через сокет и ожидает ответ
pub(super) fn sendAndReceive(socket: &mut UnixStream, bytes: &[u8]) -> io::Result<Vec<u8>>
{
  writeMessage(socket, bytes)?;
  readMessage(socket)
}

/// Супервизор: блокируется на смерти текущей Зиготы (waitpid) и пересоздаёт её.
/// Отдельный поток — поэтому spawnZygote() внутри обязан идти через Command, не через fork().
fn supervisorLoop() -> ()
{
  loop
  {
    let pidToWait: u32 = {
      let mutex: &Mutex<ZygoteHandle> = match ZygoteState.get() { Some(m) => m, None => return };
      mutex.lock().unwrap().process.id()
    };
    unsafe { libc::waitpid(pidToWait as libc::pid_t, std::ptr::null_mut(), 0); }

    let mutex: &Mutex<ZygoteHandle> = ZygoteState.get().unwrap();
    let mut guard: MutexGuard<ZygoteHandle> = mutex.lock().unwrap();
    if guard.process.id() == pidToWait // ещё не пересоздана параллельно через call()
    {
      match spawnZygote()
      {
        Ok(newHandle) => { *guard = newHandle; }
        Err(_) => { drop(guard); thread::sleep(std::time::Duration::from_millis(200)); }
      }
    }
    //
  }
}

// =================================================================================================

/// Записывает сообщение в сокет с добавлением размера данных
fn writeMessage(socket: &mut UnixStream, data: &[u8]) -> io::Result<()>
{
  socket.write_all(&(data.len() as u32).to_le_bytes())?;
  socket.write_all(data)
}

/// Читает сообщение из сокета по длине, указанной в заголовке
fn readMessage(socket: &mut UnixStream) -> io::Result<Vec<u8>>
{
  let mut lengthBuffer: [u8; 4] = [0u8; 4];
  socket.read_exact(&mut lengthBuffer)?;
  let mut buffer: Vec<u8> = vec![0u8; u32::from_le_bytes(lengthBuffer) as usize];
  socket.read_exact(&mut buffer)?;
  Ok(buffer)
}

/// Сериализует значение в байтовое представление
pub(super) fn encode<T: Serialize>(value: &T) -> Vec<u8> 
{
  let config: Configuration = bincode::config::standard();
  bincode::serde::encode_to_vec(value, config).expect("encode failed")
}
/// Десериализует байтовое представление обратно в значение
pub(super) fn decode<T: for<'a> Deserialize<'a>>(bytes: &[u8]) -> Result<T, bincode::error::DecodeError> 
{
  let config: Configuration = bincode::config::standard();
  bincode::serde::decode_from_slice(bytes, config).map(|(decoded, _bytes_read)| decoded)
}

// =================================================================================================

/// Передает сокет-дескриптор в другой процесс через анонимный канал
fn sendFd(socket: &mut UnixStream, fd: RawFd) -> io::Result<()> 
{
  // По стандарту POSIX для передачи cmsg нужен хотя бы 1 байт реальных данных
  let mut msgHeader: libc::msghdr = unsafe { std::mem::zeroed() };
  let mut dummyByte: [u8; 1] = [0u8; 1];

  let mut ioVector: libc::iovec = libc::iovec {
    iov_base: dummyByte.as_mut_ptr() as *mut _,
    iov_len: 1,
  };

  // Выделяем память под служебное сообщение и упаковываем FD в структуру SCM_RIGHTS
  let cmsgSpace: u32 = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) };
  let mut cmsgBuffer: Vec<u8> = vec![0u8; cmsgSpace as usize];

  msgHeader.msg_iov = &mut ioVector;
  msgHeader.msg_iovlen = 1;
  msgHeader.msg_control = cmsgBuffer.as_mut_ptr() as *mut _;
  msgHeader.msg_controllen = cmsgBuffer.len() as _;

  unsafe {
    let cmsg: *mut libc::cmsghdr = libc::CMSG_FIRSTHDR(&msgHeader);
    (*cmsg).cmsg_level = libc::SOL_SOCKET;
    (*cmsg).cmsg_type = libc::SCM_RIGHTS;
    (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;

    let fdPtr: *mut RawFd = libc::CMSG_DATA(cmsg) as *mut RawFd;
    fdPtr.write_unaligned(fd);
  }

  // Отправляем управляющий пакет через системный вызов ядра
  let result: libc::ssize_t = unsafe { libc::sendmsg(socket.as_raw_fd(), &msgHeader, 0) };
  if result < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Принимает сокет-дескриптор напрямую из памяти другого процесса
fn recvFd(socket: &mut UnixStream) -> io::Result<RawFd> 
{
  // Готовим буферы для приема байта-пустышки и служебного заголовка
  let mut msgHeader: libc::msghdr = unsafe { std::mem::zeroed() };
  let mut dummyByte: [u8; 1] = [0u8; 1];

  let mut ioVector: libc::iovec = libc::iovec {
    iov_base: dummyByte.as_mut_ptr() as *mut _,
    iov_len: 1,
  };

  let cmsgSpace: u32 = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) };
  let mut cmsgBuffer: Vec<u8> = vec![0u8; cmsgSpace as usize];

  msgHeader.msg_iov = &mut ioVector;
  msgHeader.msg_iovlen = 1;
  msgHeader.msg_control = cmsgBuffer.as_mut_ptr() as *mut _;
  msgHeader.msg_controllen = cmsgBuffer.len() as _;

  // Читаем сообщение из сокета
  let result: libc::ssize_t = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut msgHeader as *mut _, 0) };
  if result <= 0 { return Err(io::Error::last_os_error()); }

  // Проверяем наличие прав доступа и извлекаем полученный дескриптор
  unsafe {
    let cmsg: *mut libc::cmsghdr = libc::CMSG_FIRSTHDR(&msgHeader);
    if cmsg.is_null() || (*cmsg).cmsg_type != libc::SCM_RIGHTS {
      return Err(io::Error::new(io::ErrorKind::InvalidData, "No FD received"));
    }

    let fdPtr: *const RawFd = libc::CMSG_DATA(cmsg) as *const RawFd;
    Ok(fdPtr.read_unaligned())
  }
}

// =================================================================================================