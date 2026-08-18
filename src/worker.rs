use std::any::Any;
use libloading::Library;
use libffi::middle::{Arg, Cif, CodePtr};
use std::ffi::c_void;
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

/// Выполняется ВНУТРИ форкнутого Зиготой воркера, не самой Зиготой;
/// делает dlopen конкретной библиотеки и вызывает функцию через libffi;
/// вызывается один раз на запрос, после чего воркер завершается.
pub fn executeFFI(request: FFIRequest) -> Result<Value, String>
{
  let FFIRequest{ libraryPath, functionName, args, resultType: ffiResultType } = request;

  // ---- этот код выполняется в воркере, форкнутом от Зиготы ----
  // (все ресурсы будут автоматически освобождены при завершении процесса)

  // Загружаем библиотеку
  let library: Library = unsafe {
    Library::new(&libraryPath)
      .map_err(|e| format!("Failed to load library: {}", e))?
  };

  // Получаем указатель на функцию
  let functionPointer: *mut c_void = unsafe {
    *library
      .get::<*mut c_void>(functionName.as_bytes())
      .map_err(|e| format!("Failed to find function: {}", e))?
  };

  // Строим типы аргументов для CIF
  let argsTypes: Vec<libffi::middle::Type> = args
    .iter()
    .map(|arg| match arg {
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
    })
    .collect::<Result<Vec<_>, _>>()?;

  let returnType: libffi::middle::Type = match ffiResultType {
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
  };

  let cif: Cif = Cif::new(argsTypes, returnType);

  // Подготавливаем хранилище для значений, на которые будут ссылаться аргументы
  let mut storage: Vec<Box<dyn Any>> = Vec::with_capacity(args.len());
  for arg in &args
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
        // Для байтового вектора передаём указатель на данные
        let mut vec: Vec<u8> = v.clone();
        let pointer: *mut c_void = vec.as_mut_ptr() as *mut c_void;
        storage.push(Box::new((vec, pointer)));
      }
      Value::None => return Err("Cannot pass None".to_string()),
    }
  }

  // Строим список аргументов для libffi
  let mut argsFfi: Vec<Arg> = Vec::with_capacity(args.len());
  for (i, arg) in args.iter().enumerate() 
  {
    match arg 
    {
      Value::U8(_) => {
        let val = storage[i].downcast_ref::<u8>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::U16(_) => {
        let val = storage[i].downcast_ref::<u16>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::U32(_) => {
        let val = storage[i].downcast_ref::<u32>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::U64(_) => {
        let val = storage[i].downcast_ref::<u64>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::Usize(_) => {
        let val = storage[i].downcast_ref::<usize>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::I8(_) => {
        let val = storage[i].downcast_ref::<i8>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::I16(_) => {
        let val = storage[i].downcast_ref::<i16>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::I32(_) => {
        let val = storage[i].downcast_ref::<i32>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::I64(_) => {
        let val = storage[i].downcast_ref::<i64>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::Isize(_) => {
        let val = storage[i].downcast_ref::<isize>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::F32(_) => {
        let val = storage[i].downcast_ref::<f32>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::F64(_) => {
        let val = storage[i].downcast_ref::<f64>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::Bool(_) => {
        let val = storage[i].downcast_ref::<u8>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      Value::ByteVector(_) => {
        let (_, ptr) = storage[i]
          .downcast_ref::<(Vec<u8>, *mut c_void)>()
          .unwrap();
        argsFfi.push(Arg::new(ptr));
      }
      Value::None => return Err("Cannot pass None".to_string()),
    }
  }

  // Вызов функции
  let codePointer: CodePtr = CodePtr(functionPointer);
  let ffiResult: Value = match ffiResultType {
    Type::None => {
      unsafe { cif.call::<()>(codePointer, &argsFfi) };
      Value::None
    }
    Type::U8 => {
      let val: u8 = unsafe { cif.call::<u8>(codePointer, &argsFfi) };
      Value::U8(val)
    }
    Type::U16 => {
      let val: u16 = unsafe { cif.call::<u16>(codePointer, &argsFfi) };
      Value::U16(val)
    }
    Type::U32 => {
      let val: u32 = unsafe { cif.call::<u32>(codePointer, &argsFfi) };
      Value::U32(val)
    }
    Type::U64 => {
      let val: u64 = unsafe { cif.call::<u64>(codePointer, &argsFfi) };
      Value::U64(val)
    }
    Type::Usize => {
      let val: usize = unsafe { cif.call::<usize>(codePointer, &argsFfi) };
      Value::Usize(val)
    }
    Type::I8 => {
      let val: i8 = unsafe { cif.call::<i8>(codePointer, &argsFfi) };
      Value::I8(val)
    }
    Type::I16 => {
      let val: i16 = unsafe { cif.call::<i16>(codePointer, &argsFfi) };
      Value::I16(val)
    }
    Type::I32 => {
      let val: i32 = unsafe { cif.call::<i32>(codePointer, &argsFfi) };
      Value::I32(val)
    }
    Type::I64 => {
      let val: i64 = unsafe { cif.call::<i64>(codePointer, &argsFfi) };
      Value::I64(val)
    }
    Type::Isize => {
      let val: isize = unsafe { cif.call::<isize>(codePointer, &argsFfi) };
      Value::Isize(val)
    }
    Type::F32 => {
      let val: f32 = unsafe { cif.call::<f32>(codePointer, &argsFfi) };
      Value::F32(val)
    }
    Type::F64 => {
      let val: f64 = unsafe { cif.call::<f64>(codePointer, &argsFfi) };
      Value::F64(val)
    }
    Type::Bool => {
      let val: u8 = unsafe { cif.call::<u8>(codePointer, &argsFfi) };
      Value::Bool(val != 0)
    }
    Type::Pointer => {
      // Для указателей возвращаем None todo пока не поддерживаем
      Value::None
    }
  };

  Ok(ffiResult)
}