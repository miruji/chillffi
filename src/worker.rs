use std::any::Any;
use libloading::Library;
use libffi::middle::{Arg, Cif, CodePtr, Type};
use std::ffi::c_void;
use crate::types::{FFIType, FFIValue};
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
  args: Vec<FFIValue>,
  resultType: FFIType,
) -> Result<FFIValue, String>
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
pub fn executeFFI(request: FFIRequest) -> Result<FFIValue, String>
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
  let argsTypes: Vec<Type> = args
    .iter()
    .map(|arg| match arg {
      FFIValue::U8(_) => Ok(Type::u8()),
      FFIValue::U16(_) => Ok(Type::u16()),
      FFIValue::U32(_) => Ok(Type::u32()),
      FFIValue::U64(_) => Ok(Type::u64()),
      FFIValue::Usize(_) => Ok(Type::usize()),
      FFIValue::I8(_) => Ok(Type::i8()),
      FFIValue::I16(_) => Ok(Type::i16()),
      FFIValue::I32(_) => Ok(Type::i32()),
      FFIValue::I64(_) => Ok(Type::i64()),
      FFIValue::Isize(_) => Ok(Type::isize()),
      FFIValue::F32(_) => Ok(Type::f32()),
      FFIValue::F64(_) => Ok(Type::f64()),
      FFIValue::Bool(_) => Ok(Type::u8()),
      FFIValue::ByteVector(_) => Ok(Type::pointer()),
      FFIValue::None => Err("Cannot pass None as argument".to_string()),
    })
    .collect::<Result<Vec<_>, _>>()?;

  let returnType: Type = match ffiResultType {
    FFIType::None => Type::void(),
    FFIType::U8 => Type::u8(),
    FFIType::U16 => Type::u16(),
    FFIType::U32 => Type::u32(),
    FFIType::U64 => Type::u64(),
    FFIType::Usize => Type::usize(),
    FFIType::I8 => Type::i8(),
    FFIType::I16 => Type::i16(),
    FFIType::I32 => Type::i32(),
    FFIType::I64 => Type::i64(),
    FFIType::Isize => Type::isize(),
    FFIType::F32 => Type::f32(),
    FFIType::F64 => Type::f64(),
    FFIType::Bool => Type::u8(),
    FFIType::Pointer => Type::pointer(),
  };

  let cif: Cif = Cif::new(argsTypes.into_iter(), returnType);

  // Подготавливаем хранилище для значений, на которые будут ссылаться аргументы
  let mut storage: Vec<Box<dyn Any>> = Vec::with_capacity(args.len());
  for arg in &args
  {
    match arg
    {
      FFIValue::U8(v) => storage.push(Box::new(*v)),
      FFIValue::U16(v) => storage.push(Box::new(*v)),
      FFIValue::U32(v) => storage.push(Box::new(*v)),
      FFIValue::U64(v) => storage.push(Box::new(*v)),
      FFIValue::Usize(v) => storage.push(Box::new(*v)),
      FFIValue::I8(v) => storage.push(Box::new(*v)),
      FFIValue::I16(v) => storage.push(Box::new(*v)),
      FFIValue::I32(v) => storage.push(Box::new(*v)),
      FFIValue::I64(v) => storage.push(Box::new(*v)),
      FFIValue::Isize(v) => storage.push(Box::new(*v)),
      FFIValue::F32(v) => storage.push(Box::new(*v)),
      FFIValue::F64(v) => storage.push(Box::new(*v)),
      FFIValue::Bool(b) => storage.push(Box::new(if *b { 1u8 } else { 0u8 })),
      FFIValue::ByteVector(v) => {
        // Для байтового вектора передаём указатель на данные
        let mut vec: Vec<u8> = v.clone();
        let pointer: *mut c_void = vec.as_mut_ptr() as *mut c_void;
        storage.push(Box::new((vec, pointer)));
      }
      FFIValue::None => return Err("Cannot pass None".to_string()),
    }
  }

  // Строим список аргументов для libffi
  let mut argsFfi: Vec<Arg> = Vec::with_capacity(args.len());
  for (i, arg) in args.iter().enumerate() {
    match arg {
      FFIValue::U8(_) => {
        let val = storage[i].downcast_ref::<u8>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      FFIValue::U16(_) => {
        let val = storage[i].downcast_ref::<u16>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      FFIValue::U32(_) => {
        let val = storage[i].downcast_ref::<u32>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      FFIValue::U64(_) => {
        let val = storage[i].downcast_ref::<u64>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      FFIValue::Usize(_) => {
        let val = storage[i].downcast_ref::<usize>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      FFIValue::I8(_) => {
        let val = storage[i].downcast_ref::<i8>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      FFIValue::I16(_) => {
        let val = storage[i].downcast_ref::<i16>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      FFIValue::I32(_) => {
        let val = storage[i].downcast_ref::<i32>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      FFIValue::I64(_) => {
        let val = storage[i].downcast_ref::<i64>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      FFIValue::Isize(_) => {
        let val = storage[i].downcast_ref::<isize>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      FFIValue::F32(_) => {
        let val = storage[i].downcast_ref::<f32>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      FFIValue::F64(_) => {
        let val = storage[i].downcast_ref::<f64>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      FFIValue::Bool(_) => {
        let val = storage[i].downcast_ref::<u8>().unwrap();
        argsFfi.push(Arg::new(val));
      }
      FFIValue::ByteVector(_) => {
        let (_, ptr) = storage[i]
          .downcast_ref::<(Vec<u8>, *mut c_void)>()
          .unwrap();
        argsFfi.push(Arg::new(ptr));
      }
      FFIValue::None => return Err("Cannot pass None".to_string()),
    }
  }

  // Вызов функции
  let codePointer: CodePtr = CodePtr(functionPointer);
  let ffiResult: FFIValue = match ffiResultType {
    FFIType::None => {
      unsafe { cif.call::<()>(codePointer, &argsFfi) };
      FFIValue::None
    }
    FFIType::U8 => {
      let val: u8 = unsafe { cif.call::<u8>(codePointer, &argsFfi) };
      FFIValue::U8(val)
    }
    FFIType::U16 => {
      let val: u16 = unsafe { cif.call::<u16>(codePointer, &argsFfi) };
      FFIValue::U16(val)
    }
    FFIType::U32 => {
      let val: u32 = unsafe { cif.call::<u32>(codePointer, &argsFfi) };
      FFIValue::U32(val)
    }
    FFIType::U64 => {
      let val: u64 = unsafe { cif.call::<u64>(codePointer, &argsFfi) };
      FFIValue::U64(val)
    }
    FFIType::Usize => {
      let val: usize = unsafe { cif.call::<usize>(codePointer, &argsFfi) };
      FFIValue::Usize(val)
    }
    FFIType::I8 => {
      let val: i8 = unsafe { cif.call::<i8>(codePointer, &argsFfi) };
      FFIValue::I8(val)
    }
    FFIType::I16 => {
      let val: i16 = unsafe { cif.call::<i16>(codePointer, &argsFfi) };
      FFIValue::I16(val)
    }
    FFIType::I32 => {
      let val: i32 = unsafe { cif.call::<i32>(codePointer, &argsFfi) };
      FFIValue::I32(val)
    }
    FFIType::I64 => {
      let val: i64 = unsafe { cif.call::<i64>(codePointer, &argsFfi) };
      FFIValue::I64(val)
    }
    FFIType::Isize => {
      let val: isize = unsafe { cif.call::<isize>(codePointer, &argsFfi) };
      FFIValue::Isize(val)
    }
    FFIType::F32 => {
      let val: f32 = unsafe { cif.call::<f32>(codePointer, &argsFfi) };
      FFIValue::F32(val)
    }
    FFIType::F64 => {
      let val: f64 = unsafe { cif.call::<f64>(codePointer, &argsFfi) };
      FFIValue::F64(val)
    }
    FFIType::Bool => {
      let val: u8 = unsafe { cif.call::<u8>(codePointer, &argsFfi) };
      FFIValue::Bool(val != 0)
    }
    FFIType::Pointer => {
      // Для указателей возвращаем None todo пока не поддерживаем
      FFIValue::None
    }
  };

  Ok(ffiResult)
}