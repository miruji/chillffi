pub mod worker;
pub mod types;
pub mod zygote;
// =================================================================================================
use std::{env, io};
use crate::zygote::{initZygote, runAsZygote, ZygoteFlag};
// =================================================================================================

/// Единая точка входа для инициализации зиготы в любом бинарнике (включая тесты).
/// Проверяет, не запущен ли процесс как зигота; если да – переключается в режим демона,
/// иначе – инициализирует родительскую сторону.
#[ctor::ctor(unsafe)]
fn zygoteEntrypoint()
{
  let mut args = env::args_os();
  args.next();
  if let Some(arg) = args.next() {
    if arg == ZygoteFlag {
      runAsZygote();
    }
  }
}

pub fn setupZygote() -> io::Result<()>
{
  initZygote()
}

// todo Тут должен прокид наружу lib