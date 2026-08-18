use std::env;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, Child};
use std::sync::{Mutex, OnceLock};
use std::thread;
use serde::{Serialize, Deserialize};
use crate::worker::executeFFI;
use crate::types::{FFIType, FFIValue, FFIRequest, FFIResponse};
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

/// Скрытый флаг запуска: если он первый аргумент — это не RTS, а процесс-Зигота.
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
  pub args: Vec<FFIValue>,
  /// todo desc
  pub resultType: FFIType
}

/// todo desc
#[derive(Serialize, Deserialize)]
pub enum FFIResponse
{
  Ok(FFIValue),
  Err(String)
}

/// todo desc
struct ZygoteHandle
{
  /// todo desc
  process: Child,
  /// todo desc
  socket: UnixStream,
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
static ZygoteState: OnceLock<Mutex<ZygoteHandle>> = OnceLock::new();

// =================================================================================================

/// Точка входа дочернего процесса-Зиготы;
/// main() обязан вызвать это первой строкой, если первый аргумент == ZygoteFlag;
/// Процесс порождён через Command (fork+exec) — интерпретатор RTS не прогревался,
/// AST не парсился, кучи метаданных нет. Библиотеку заранее НЕ грузит.
pub fn runAsZygote() -> !
{
  let socketPath: String = env::args().nth(2).expect("Zygote: missing socket path");
  let socket: UnixStream = UnixStream::connect(&socketPath).expect("Zygote: cannot connect to RTS");
  zygoteLoop(socket);
}

/// Инициализация Зиготы; вызывать один раз, самой первой строкой обычного main(),
/// до args-парсинга, до чтения файла, до parseLines/readTokens.
pub fn initZygote() -> io::Result<()>
{
  let handle: ZygoteHandle = spawnZygote()?;
  ZygoteState.set(Mutex::new(handle))
    .map_err(|_| io::Error::new(io::ErrorKind::AlreadyExists, "Zygote already initialized"))?;
  thread::spawn(supervisorLoop);
  Ok(())
}

/// Зигота порождается ТОЛЬКО через Command (fork+exec) — и при старте, и при пересоздании.
/// Это принципиально: обычный fork() пересоздания из уже прогретого многопоточного RTS
/// (супервизор — отдельный поток) унаследовал бы чужие мьютексы в захваченном состоянии —
/// та самая дедлок-ловушка из твоего разбора. exec() полностью заменяет образ процесса,
/// поэтому Зигота всегда рождается чистой, независимо от того, насколько "толстым"
/// успел стать RTS к моменту respawn'а.
fn spawnZygote() -> io::Result<ZygoteHandle>
{
  let uniqueId: u128 = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
  let socketPath: PathBuf = env::temp_dir()
    .join(format!("rts-zygote-{}-{}.sock", std::process::id(), uniqueId));
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

/// Тело Зиготы: бесконечный цикл ожидания команд.
/// Библиотеку заранее НЕ грузит — язык интерпретируемый, какой FFI понадобится, неизвестно
/// заранее. Зигота — пустой рантайм-шаблон; dlopen делает только форкнутый воркер.
fn zygoteLoop(mut socket: UnixStream) -> !
{
  unsafe { libc::signal(libc::SIGCHLD, libc::SIG_IGN); } // авто-reap воркеров, без зомби

  loop
  {
    let requestBytes: Vec<u8> = match readMessage(&mut socket)
    {
      Ok(bytes) => bytes,
      Err(_) => std::process::exit(0), // сокет закрылся — RTS завершился, Зигота умирает вместе с ним
    };

    match unsafe { libc::fork() }
    {
      -1 =>
      {
        let _ = writeMessage(&mut socket, &encode(&FFIResponse::Err("Zygote fork failed".into())));
      }
      0 =>
      { // Воркер: разовый, форкнут от пустой Зиготы — почти нулевой page-fault
        let response: FFIResponse = handleRequest(&requestBytes);
        let _ = writeMessage(&mut socket, &encode(&response));
        std::process::exit(0);
      }
      _ => {} // Зигота: воркера не ждёт, сразу слушает следующий запрос
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
  let mut guard = mutex.lock().unwrap();

  if let Ok(responseBytes) = sendAndReceive(&mut guard.socket, &bytes)
  {
    return decode(&responseBytes).map_err(|e| e.to_string());
  }

  *guard = spawnZygote().map_err(|e| format!("Zygote respawn failed: {}", e))?;
  let responseBytes: Vec<u8> = sendAndReceive(&mut guard.socket, &bytes).map_err(|e| e.to_string())?;
  decode(&responseBytes).map_err(|e| e.to_string())
}

/// todo desc
fn sendAndReceive(socket: &mut UnixStream, bytes: &[u8]) -> io::Result<Vec<u8>>
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
    let mut guard = mutex.lock().unwrap();
    if guard.process.id() == pidToWait // ещё не пересоздана параллельно через call()
    {
      match spawnZygote()
      {
        Ok(newHandle) => { *guard = newHandle; }
        Err(_) => { drop(guard); thread::sleep(std::time::Duration::from_millis(200)); }
      }
    }
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
  let mut lenBuf: [u8; 4] = [0u8; 4];
  socket.read_exact(&mut lenBuf)?;
  let mut buffer: Vec<u8> = vec![0u8; u32::from_le_bytes(lenBuf) as usize];
  socket.read_exact(&mut buffer)?;
  Ok(buffer)
}

/// todo desc
fn encode<T: Serialize>(value: &T) -> Vec<u8> {
  let config = bincode::config::standard();
  bincode::serde::encode_to_vec(value, config).expect("encode failed")
}
/// todo desc
fn decode<T: for<'a> Deserialize<'a>>(bytes: &[u8]) -> Result<T, bincode::error::DecodeError> {
  let config = bincode::config::standard();
  bincode::serde::decode_from_slice(bytes, config).map(|(decoded, _bytes_read)| decoded)
}

// =================================================================================================