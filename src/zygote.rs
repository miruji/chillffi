use std::cell::RefCell;
use std::env;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, Child};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use bincode::config::Configuration;
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

/// Запрос на выполнение FFI, уходящий в Зиготу целиком (она не знает про Token/StructureType).
#[derive(Serialize, Deserialize)]
pub struct FFIRequest
{
  /// todo desc
  pub libraryPath: String,
  /// todo desc
  pub functionName: String,
  /// todo desc
  pub args: Vec<Value>,
  /// todo desc
  pub resultType: Type
}

/// todo desc
#[derive(Serialize, Deserialize)]
pub enum FFIResponse
{
  Ok(Value),
  Err(String)
}

/// todo desc
pub(super) struct ZygoteHandle
{
  /// todo desc
  process: Child,
  /// todo desc
  pub(super) socket: UnixStream,
  /// todo desc
  socketPath: PathBuf
}

impl Drop for ZygoteHandle
{
  /// todo desc
  fn drop(&mut self)
  {
    let _ = self.process.kill();
    let _ = std::fs::remove_file(&self.socketPath);
  }
}

/// todo desc
pub(super) static ZygoteState: OnceLock<Mutex<ZygoteHandle>> = OnceLock::new();

// =================================================================================================

/// todo desc
pub struct ClonedZygote 
{
  /// todo desc
  pub pid: libc::pid_t,
  /// todo desc
  pub socket: UnixStream,
}

/// todo desc
impl ClonedZygote 
{
  /// Запрашивает клон у Главной Зиготы и возвращает его RAII-хэндл
  pub fn getMeClone() -> io::Result<Self> {
    let mutex = ZygoteState.get().expect("Zygote not initialized");
    let mut guard = mutex.lock().unwrap();

    let uniqueId = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let cloneSockPath = env::temp_dir().join(format!("zygote-clone-{}.sock", uniqueId));

    let listener = UnixListener::bind(&cloneSockPath)?;

    // 1. Передаем Главной Зиготе путь сокета, куда должен подключиться её клон
    writeMessage(&mut guard.socket, cloneSockPath.to_str().unwrap().as_bytes())?;

    // 2. Получаем PID форкнутого клона
    let pidBytes = readMessage(&mut guard.socket)?;
    let pid = i32::from_le_bytes(pidBytes.try_into().unwrap());

    // 3. Принимаем подключение от клона
    let (socket, _) = listener.accept()?;
    let _ = std::fs::remove_file(&cloneSockPath);

    Ok(ClonedZygote { pid, socket })
  }

  /// Вызов FFI внутри конкретного клона
  pub fn call(&mut self, request: FFIRequest) -> Result<FFIResponse, String> {
    let bytes = encode(&request);
    let responseBytes = sendAndReceive(&mut self.socket, &bytes).map_err(|e| e.to_string())?;
    decode(&responseBytes).map_err(|e| e.to_string())
  }
}

// При drop() клон моментально убивается, Главная Зигота не затрагивается
impl Drop for ClonedZygote {
  fn drop(&mut self) {
    unsafe {
      libc::kill(self.pid, libc::SIGKILL);
      libc::waitpid(self.pid, std::ptr::null_mut(), libc::WNOHANG);
    }
  }
}

// =================================================================================================

thread_local!{
  /// Стек зигот: каждый вход в ffi!{} кладет новую зиготу наверх стека
  pub(crate) static ZygoteStack: RefCell<Vec<ClonedZygote>> = const { RefCell::new(Vec::new()) };
}

/// todo desc
pub struct ZygoteGuard;

impl ZygoteGuard 
{
  /// todo desc
  pub fn enter(zygote: ClonedZygote) -> Self {
    ZygoteStack.with(|stack| {
      stack.borrow_mut().push(zygote);
    });
    ZygoteGuard
  }
}

impl Drop for ZygoteGuard 
{
  /// Снимаем со стека именно эту зиготу,
  /// и Rust автоматически вызывает её drop(), убивая процесс.
  fn drop(&mut self) 
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
  let socketPath: String = env::args().nth(2).expect("Zygote: missing socket path");
  let socket: UnixStream = UnixStream::connect(&socketPath).expect("Zygote: cannot connect to runtime");
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
  let uniqueId: u128 = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
  let socketPath: PathBuf = env::temp_dir()
    .join(format!("runtime-zygote-{}-{}.sock", std::process::id(), uniqueId)); // todo Какая-то фигня с .sock?
  let _ = std::fs::remove_file(&socketPath);

  let listener: UnixListener = UnixListener::bind(&socketPath)?;

  let currentExe: PathBuf = env::current_exe()?;
  let process: Child = Command::new(currentExe)
    .arg(ZygoteFlag)
    .arg(&socketPath)
    .spawn()?;

  let (socket, _addr) = listener.accept()?;

  Ok(ZygoteHandle{ process, socket, socketPath })
}

/// Цикл главной зиготы: бесконечный цикл ожидания команд.
/// Библиотеку заранее НЕ грузит — язык интерпретируемый, какой FFI понадобится, неизвестно
/// заранее. Зигота — пустой рантайм-шаблон; dlopen делает только форкнутый зигота.
fn zygoteLoop(mut socket: UnixStream) -> !
{
  unsafe { libc::signal(libc::SIGCHLD, libc::SIG_IGN); } // Авто-reap зигот, без зомби

  loop {
    // Получаем путь к сокету для нового клона
    let msg: Vec<u8> = match readMessage(&mut socket) 
    {
      Ok(bytes) => bytes,
      Err(_) => std::process::exit(0), // Parent умер
    };

    let cloneSockPath: String = String::from_utf8_lossy(&msg).to_string();

    match unsafe { libc::fork() } 
    {
      -1 => {
        let _ = writeMessage(&mut socket, &0i32.to_le_bytes());
      }
      0 => {
        // --- КЛОНЗИГОТА ---
        if let Ok(cloneSocket) = UnixStream::connect(&cloneSockPath) {
          cloneLoop(cloneSocket);
        }
        std::process::exit(0);
      }
      pid => {
        // --- ГЛАВНАЯ ЗИГОТА ---
        // Возвращает PID клона рантайму и сразу ждёт следующий сигнал на клон
        let _ = writeMessage(&mut socket, &pid.to_le_bytes());
      }
    }
    //
  }
}

/// Персональный цикл клона
fn cloneLoop(mut socket: UnixStream) -> ! 
{
  loop 
  {
    let requestBytes: Vec<u8> = match readMessage(&mut socket) 
    {
      Ok(bytes) => bytes,
      Err(_) => std::process::exit(0), // Переменную drop'нули — сокет закрылся, клон ушел
    };

    let response: FFIResponse = handleRequest(&requestBytes);
    if writeMessage(&mut socket, &encode(&response)).is_err() {
      std::process::exit(0);
    }
  }
}

/// todo desc
fn handleRequest(requestBytes: &[u8]) -> FFIResponse
{
  match decode::<FFIRequest>(requestBytes)
  {
    Ok(request) => match executeFFI(request)
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

/// todo desc
pub(super) fn sendAndReceive(socket: &mut UnixStream, bytes: &[u8]) -> io::Result<Vec<u8>>
{
  writeMessage(socket, bytes)?;
  readMessage(socket)
}

/// Супервизор: блокируется на смерти текущей Зиготы (waitpid) и пересоздаёт её.
/// Отдельный поток — поэтому spawnZygote() внутри обязан идти через Command, не через fork().
fn supervisorLoop()
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

/// todo desc
fn writeMessage(socket: &mut UnixStream, data: &[u8]) -> io::Result<()>
{
  socket.write_all(&(data.len() as u32).to_le_bytes())?;
  socket.write_all(data)
}

/// todo desc
fn readMessage(socket: &mut UnixStream) -> io::Result<Vec<u8>>
{
  let mut lengthBuffer: [u8; 4] = [0u8; 4];
  socket.read_exact(&mut lengthBuffer)?;
  let mut buffer: Vec<u8> = vec![0u8; u32::from_le_bytes(lengthBuffer) as usize];
  socket.read_exact(&mut buffer)?;
  Ok(buffer)
}

/// todo desc
pub(super) fn encode<T: Serialize>(value: &T) -> Vec<u8> 
{
  let config: Configuration = bincode::config::standard();
  bincode::serde::encode_to_vec(value, config).expect("encode failed")
}
/// todo desc
pub(super) fn decode<T: for<'a> Deserialize<'a>>(bytes: &[u8]) -> Result<T, bincode::error::DecodeError> 
{
  let config: Configuration = bincode::config::standard();
  bincode::serde::decode_from_slice(bytes, config).map(|(decoded, _bytes_read)| decoded)
}

// =================================================================================================