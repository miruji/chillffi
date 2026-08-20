use std::any::Any;
use libloading::Library;
use libffi::middle::{Arg, Cif, CodePtr};
use std::ffi::c_void;
use fxhash::FxHashMap;
use crate::ffi::value::{Type, Value};
use crate::zygote;
use crate::zygote::{FFIRequest, FFIResponse};
// =================================================================================================

/// Формирует запрос и отправляет его Зиготе;
/// сама эта функция ничего не форкает и не грузит — только сериализация и IPC;
///
/// Принимает путь к библиотеке, имя функции, аргументы в виде токенов и ожидаемый тип результата;
///
/// Возвращает результат как FFIValue или ошибку.
pub fn callExternal(
  libraryPath: &str,
  methodName: &str,
  args: Vec<Value>,
  resultType: Type,
) -> Result<Value, String>
{
  // Собираем запрос и отправляем Зиготе
  let request: FFIRequest = FFIRequest {
    libraryPath:  libraryPath.to_string(),
    functionName: methodName.to_string(),
    args,
    resultType,
  };

  match zygote::call(request)?
  {
    FFIResponse::Ok(value) => Ok(value),
    FFIResponse::Err(e)    => Err(e),
  }
}

// =================================================================================================

impl TryFrom<&Value> for libffi::middle::Type {
  type Error = String;

  /// Определяет тип аргумента для C-функции 
  /// и сразу отсекает None, который нельзя передать.
  fn try_from(val: &Value) -> Result<Self, Self::Error> 
  {
    match val 
    {
      Value::U8(_) => Ok(libffi::middle::Type::u8()),
      Value::U16(_) => Ok(libffi::middle::Type::u16()),
      Value::U32(_) => Ok(libffi::middle::Type::u32()),
      Value::U64(_) => Ok(libffi::middle::Type::u64()),
      Value::Usize(_) => Ok(libffi::middle::Type::usize()),
      Value::I8(_) => Ok(libffi::middle::Type::i8()),
      Value::I16(_) => Ok(libffi::middle::Type::i16()),
      Value::I32(_) => Ok(libffi::middle::Type::i32()),
      Value::I64(_) => Ok(libffi::middle::Type::i64()),
      Value::Isize(_) => Ok(libffi::middle::Type::isize()),
      Value::F32(_) => Ok(libffi::middle::Type::f32()),
      Value::F64(_) => Ok(libffi::middle::Type::f64()),
      Value::Bool(_) => Ok(libffi::middle::Type::u8()),
      Value::ByteVector(_) => Ok(libffi::middle::Type::pointer()),
      Value::None => Err("Cannot pass None as argument".to_string()),
    }
  }
}

impl From<&Type> for libffi::middle::Type 
{
  /// Задает тип возвращаемого значения, чтобы libffi знала, 
  /// сколько байт читать после вызова.
  fn from(t: &Type) -> Self 
  {
    match t 
    {
      Type::None => libffi::middle::Type::void(),
      Type::U8 => libffi::middle::Type::u8(),
      Type::U16 => libffi::middle::Type::u16(),
      Type::U32 => libffi::middle::Type::u32(),
      Type::U64 => libffi::middle::Type::u64(),
      Type::Usize => libffi::middle::Type::usize(),
      Type::I8 => libffi::middle::Type::i8(),
      Type::I16 => libffi::middle::Type::i16(),
      Type::I32 => libffi::middle::Type::i32(),
      Type::I64 => libffi::middle::Type::i64(),
      Type::Isize => libffi::middle::Type::isize(),
      Type::F32 => libffi::middle::Type::f32(),
      Type::F64 => libffi::middle::Type::f64(),
      Type::Bool => libffi::middle::Type::u8(),
      Type::Pointer => libffi::middle::Type::pointer(),
    }
  }
}

/// Удерживает аргументы в буфере storage, 
/// чтобы они не удалились из памяти во время C-вызова, 
/// и собирает на них указатели.
fn prepareFFIArgs<'a>(
  args: &'a [Value],
  storage: &'a mut Vec<Box<dyn Any>>,
) -> Result<Vec<Arg<'a>>, String> 
{
  // Подготавливаем хранилище для значений, 
  // на которые будут ссылаться аргументы.
  for arg in args 
  {
    match arg 
    {
      Value::U8(v) => storage.push(Box::new(*v)),
      Value::U16(v) => storage.push(Box::new(*v)),
      Value::U32(v) => storage.push(Box::new(*v)),
      Value::U64(v) => storage.push(Box::new(*v)),
      Value::Usize(v) => storage.push(Box::new(*v)),
      Value::I8(v) => storage.push(Box::new(*v)),
      Value::I16(v) => storage.push(Box::new(*v)),
      Value::I32(v) => storage.push(Box::new(*v)),
      Value::I64(v) => storage.push(Box::new(*v)),
      Value::Isize(v) => storage.push(Box::new(*v)),
      Value::F32(v) => storage.push(Box::new(*v)),
      Value::F64(v) => storage.push(Box::new(*v)),
      Value::Bool(b) => storage.push(Box::new(if *b { 1u8 } else { 0u8 })),
      Value::ByteVector(v) => {
        // Для байтового вектора передаём указатель на данные;
        // Важно: Если C-коду надо будет его на долгое время - это его забота.
        // Указатель будет удален нами, потому что он временный + работа процесса зиготы.
        let mut vec: Vec<u8> = v.clone();
        let pointer: *mut c_void = vec.as_mut_ptr() as *mut c_void;
        storage.push(Box::new((vec, pointer)));
      }
      Value::None => return Err("Cannot pass None".to_string()),
    }
  }

  // Строим список аргументов для libffi
  let mut argsFfi: Vec<Arg<'a>> = Vec::with_capacity(args.len());
  for (i, arg) in args.iter().enumerate() 
  {
    match arg 
    {
      Value::U8(_) => {
        let val: &u8 = storage[i].downcast_ref::<u8>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::U16(_) => {
        let val: &u16 = storage[i].downcast_ref::<u16>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::U32(_) => {
        let val: &u32 = storage[i].downcast_ref::<u32>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::U64(_) => {
        let val: &u64 = storage[i].downcast_ref::<u64>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::Usize(_) => {
        let val: &usize = storage[i].downcast_ref::<usize>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::I8(_) => {
        let val: &i8 = storage[i].downcast_ref::<i8>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::I16(_) => {
        let val: &i16 = storage[i].downcast_ref::<i16>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::I32(_) => {
        let val: &i32 = storage[i].downcast_ref::<i32>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::I64(_) => {
        let val: &i64 = storage[i].downcast_ref::<i64>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::Isize(_) => {
        let val: &isize = storage[i].downcast_ref::<isize>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::F32(_) => {
        let val: &f32 = storage[i].downcast_ref::<f32>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::F64(_) => {
        let val: &f64 = storage[i].downcast_ref::<f64>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::Bool(_) => {
        let val: &u8 = storage[i].downcast_ref::<u8>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::ByteVector(_) => {
        let (_, ptr): &(Vec<u8>, *mut c_void) = storage[i]
          .downcast_ref::<(Vec<u8>, *mut c_void)>()
          .unwrap();
        argsFfi.push(Arg::new(ptr));
      }
      Value::None => return Err("Cannot pass None".to_string()),
    }
  }

  Ok(argsFfi)
}

/// Вызывает C-функцию по указателю 
/// и оборачивает полученный сырой результат обратно в enum Value.
fn invokeFFI(cif: &Cif, codePointer: CodePtr, argsFfi: &[Arg], ffiResultType: &Type) -> Value 
{
  match ffiResultType 
  {
    Type::None => {
      unsafe { cif.call::<()>(codePointer, argsFfi) };
      Value::None
    }
    Type::U8 => {
      let val: u8 = unsafe { cif.call::<u8>(codePointer, argsFfi) };
      Value::U8(val)
    }
    Type::U16 => {
      let val: u16 = unsafe { cif.call::<u16>(codePointer, argsFfi) };
      Value::U16(val)
    }
    Type::U32 => {
      let val: u32 = unsafe { cif.call::<u32>(codePointer, argsFfi) };
      Value::U32(val)
    }
    Type::U64 => {
      let val: u64 = unsafe { cif.call::<u64>(codePointer, argsFfi) };
      Value::U64(val)
    }
    Type::Usize => {
      let val: usize = unsafe { cif.call::<usize>(codePointer, argsFfi) };
      Value::Usize(val)
    }
    Type::I8 => {
      let val: i8 = unsafe { cif.call::<i8>(codePointer, argsFfi) };
      Value::I8(val)
    }
    Type::I16 => {
      let val: i16 = unsafe { cif.call::<i16>(codePointer, argsFfi) };
      Value::I16(val)
    }
    Type::I32 => {
      let val: i32 = unsafe { cif.call::<i32>(codePointer, argsFfi) };
      Value::I32(val)
    }
    Type::I64 => {
      let val: i64 = unsafe { cif.call::<i64>(codePointer, argsFfi) };
      Value::I64(val)
    }
    Type::Isize => {
      let val: isize = unsafe { cif.call::<isize>(codePointer, argsFfi) };
      Value::Isize(val)
    }
    Type::F32 => {
      let val: f32 = unsafe { cif.call::<f32>(codePointer, argsFfi) };
      Value::F32(val)
    }
    Type::F64 => {
      let val: f64 = unsafe { cif.call::<f64>(codePointer, argsFfi) };
      Value::F64(val)
    }
    Type::Bool => {
      let val: u8 = unsafe { cif.call::<u8>(codePointer, argsFfi) };
      Value::Bool(val != 0)
    }
    Type::Pointer => {
      // Для указателей возвращаем None todo пока не поддерживаем
      Value::None
    }
  }
}

// =================================================================================================

/// Выполняется ВНУТРИ форкнутого Зиготой воркера, не самой Зиготой;
/// делает dlopen конкретной библиотеки и вызывает функцию через libffi;
/// вызывается один раз на запрос, после чего воркер завершается.
pub fn executeFFI(request: FFIRequest, cache: &mut FxHashMap<String, Library>) -> Result<Value, String>
{
  // 1. Проверяем аргументы на наличие Value::None до сборки типов C ABI
  for (index, arg) in request.args.iter().enumerate() {
    if matches!(arg, Value::None) {
      return Err(format!("Cannot pass Value::None as argument at index {}", index));
    }
  }
  
  //
  let FFIRequest{ libraryPath, functionName, args, resultType: ffiResultType } = request;

  // Этот код выполняется в клоне от основной зиготы;
  // Все ресурсы будут автоматически освобождены при завершении процесса.

  // Достаём библиотеку из кеша или загружаем с диска при первом вызове
  if !cache.contains_key(&libraryPath) {
    let lib: Library = unsafe {
      Library::new(&libraryPath)
        .map_err(|e| format!("Failed to load library: {}", e))?
    };
    cache.insert(libraryPath.clone(), lib);
  }
  let library: &Library = cache.get(&libraryPath).unwrap();

  // Получаем указатель на функцию
  let functionPointer: *mut c_void = unsafe {
    *library
      .get::<*mut c_void>(functionName.as_bytes())
      .map_err(|e| format!("Failed to find function: {}", e))?
  };

  // Строим типы аргументов для CIF
  let argsTypes: Vec<libffi::middle::Type> = args
    .iter()
    .map(libffi::middle::Type::try_from)
    .collect::<Result<Vec<_>, _>>()?;

  let returnType: libffi::middle::Type = libffi::middle::Type::from(&ffiResultType);

  let cif: Cif = Cif::new(argsTypes, returnType);

  // Подготавливаем хранилище для значений, на которые будут ссылаться аргументы
  let mut storage: Vec<Box<dyn Any>> = Vec::with_capacity(args.len());

  // Строим список аргументов для libffi
  let argsFfi: Vec<Arg> = prepareFFIArgs(&args, &mut storage)?;

  // Вызов функции
  let codePointer: CodePtr = CodePtr(functionPointer);
  let ffiResult: Value = invokeFFI(&cif, codePointer, &argsFfi, &ffiResultType);

  Ok(ffiResult)
}

// =================================================================================================