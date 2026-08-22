use crate::ffi::errors::FFIError;
use std::any::Any;
use libloading::Library;
use libffi::middle::{Arg, Cif, CodePtr};
use std::ffi::c_void;
use fxhash::FxHashMap;
use crate::ffi::value::{Type, Value};
use crate::zygote::{FFIRequest};
// =================================================================================================

/// Maps a Value variant to its corresponding libffi C ABI type(s).
#[inline]
fn toCifTypes(val: &Value) -> Result<Vec<libffi::middle::Type>, FFIError>
{
  match val
  {
    Value::U8(_) => Ok(vec![libffi::middle::Type::u8()]),
    Value::U16(_) => Ok(vec![libffi::middle::Type::u16()]),
    Value::U32(_) => Ok(vec![libffi::middle::Type::u32()]),
    Value::U64(_) => Ok(vec![libffi::middle::Type::u64()]),
    Value::Usize(_) => Ok(vec![libffi::middle::Type::usize()]),
    Value::I8(_) => Ok(vec![libffi::middle::Type::i8()]),
    Value::I16(_) => Ok(vec![libffi::middle::Type::i16()]),
    Value::I32(_) => Ok(vec![libffi::middle::Type::i32()]),
    Value::I64(_) => Ok(vec![libffi::middle::Type::i64()]),
    Value::Isize(_) => Ok(vec![libffi::middle::Type::isize()]),
    Value::F32(_) => Ok(vec![libffi::middle::Type::f32()]),
    Value::F64(_) => Ok(vec![libffi::middle::Type::f64()]),
    Value::Bool(_) => Ok(vec![libffi::middle::Type::u8()]),
    Value::Pointer(_) => Ok(vec![libffi::middle::Type::pointer()]),
    Value::RawString(_) | Value::CString(_) => Ok(vec![libffi::middle::Type::pointer()]),
    Value::String(_) => Ok(vec![libffi::middle::Type::pointer(), libffi::middle::Type::usize()]),
    Value::None => Err(FFIError::BadArgument("Cannot pass Value::None as argument".to_string()))
  }
}

impl From<&Type> for libffi::middle::Type 
{
  /// Specifies the return value type so that libffi knows
  /// how many bytes to read after the call.
  #[inline]
  fn from(t: &Type) -> Self 
  {
    match t 
    {
      Type::None => Self::void(),
      Type::U8 => Self::u8(),
      Type::U16 => Self::u16(),
      Type::U32 => Self::u32(),
      Type::U64 => Self::u64(),
      Type::Usize => Self::usize(),
      Type::I8 => Self::i8(),
      Type::I16 => Self::i16(),
      Type::I32 => Self::i32(),
      Type::I64 => Self::i64(),
      Type::Isize => Self::isize(),
      Type::F32 => Self::f32(),
      Type::F64 => Self::f64(),
      Type::Bool => Self::u8(),
      Type::Pointer => Self::pointer(),
    }
  }
}

/// Keeps the arguments in the storage buffer,
/// so that they are not removed from memory during the C call,
/// and collects pointers to them.
/// 
/// # Memory Lifetime Constraint:
/// All heap allocations for string buffers (`RawString`, `CString`, `String`) 
/// are strictly temporary (transient) and guaranteed to be valid ONLY for the duration 
/// of the FFI call; When `prepareFFIArgs` storage goes out of scope, Rust automatically 
/// reclaims all vector allocations. If the C side needs to retain this data beyond 
/// the function execution, it must make its own deep copy (e.g., via `memcpy` or `strdup`).
fn prepareFFIArgs<'a>(
  args: &'a [Value],
  storage: &'a mut Vec<Box<dyn Any>>,
) -> Result<Vec<Arg<'a>>, FFIError> 
{
  // Prepare storage for the values
  // that the arguments will reference.
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
      Value::Pointer(addr) => {
        let ptr: *mut c_void = *addr as *mut c_void;
        storage.push(Box::new(ptr)); // Pointer
      }
      Value::RawString(v) => {
        let mut vec: Vec<u8> = v.clone();
        let pointer: *mut c_void = vec.as_mut_ptr() as *mut c_void;
        storage.push(Box::new((vec, pointer)));
      }
      Value::CString(v) => {
        let mut vec: Vec<u8> = v.clone();
        if !vec.ends_with(&[0]) { vec.push(0); } // Guarantee \0
        let pointer: *mut c_void = vec.as_mut_ptr() as *mut c_void;
        storage.push(Box::new((vec, pointer)));
      }
      Value::String(v) => {
        let mut vec: Vec<u8> = v.clone();
        let pointer: *mut c_void = vec.as_mut_ptr() as *mut c_void;
        let len: usize = vec.len();
        storage.push(Box::new((vec, pointer, len))); // Saving the length
      }
      Value::None => return Err(FFIError::BadArgument("Cannot pass Value::None".to_string()))
    }
  }

  // Build the list of arguments for libffi
  let mut argsFfi: Vec<Arg<'a>> = Vec::with_capacity(args.len());
  for (i, arg) in args.iter().enumerate() 
  {
    match arg 
    {
      Value::U8(_) => {
        let val: &u8 = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(val));
      }
      Value::U16(_) => {
        let val: &u16 = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(val));
      }
      Value::U32(_) => {
        let val: &u32 =downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(val));
      }
      Value::U64(_) => {
        let val: &u64 = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(val));
      }
      Value::Usize(_) => {
        let val: &usize = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(val));
      }
      Value::I8(_) => {
        let val: &i8 = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(val));
      }
      Value::I16(_) => {
        let val: &i16 = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(val));
      }
      Value::I32(_) => {
        let val: &i32 = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(val));
      }
      Value::I64(_) => {
        let val: &i64 = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(val));
      }
      Value::Isize(_) => {
        let val: &isize = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(val));
      }
      Value::F32(_) => {
        let val: &f32 = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(val));
      }
      Value::F64(_) => {
        let val: &f64 = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(val));
      }
      Value::Bool(_) => {
        let val: &u8 = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(val));
      }
      Value::Pointer(_) => {
        let ptr: &*mut c_void = storage[i].downcast_ref().unwrap();
        argsFfi.push(Arg::new(ptr));
      }
      Value::RawString(_) | Value::CString(_) => {
        let (_, ptr): &(Vec<u8>, *mut c_void) = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(ptr));
      }
      Value::String(_) => {
        let (_, ptr, len): &(Vec<u8>, *mut c_void, usize) = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(ptr));
        argsFfi.push(Arg::new(len));
      }
      Value::None => return Err(FFIError::BadArgument("Cannot pass Value::None".to_string()))
    }
  }

  Ok(argsFfi)
}

/// Calls the C function by pointer
/// and wraps the obtained raw result back into the Value enum.
#[inline]
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
    Type::Pointer => 
    { 
      let ptr: *mut c_void = unsafe { cif.call::<*mut c_void>(codePointer, argsFfi) };
      Value::Pointer(ptr as usize)
    }
  }
}

/// Safely downcasts a boxed storage entry to a concrete reference type.
#[inline]
fn downcastRef<T: 'static>(entry: &Box<dyn Any>) -> Result<&T, FFIError>
{
  entry.downcast_ref::<T>()
    .ok_or_else(|| FFIError::ArgumentDowncastFailed("FFI storage type mismatch".to_string()))
}

// =================================================================================================

/// Dispatches an FFI request inside the zygote clone to the appropriate handler.
pub(super) fn executeFFI(request: FFIRequest, cache: &mut FxHashMap<String, Library>) -> Result<Value, FFIError>
{
  match request
  {
    FFIRequest::Call { libraryPath, functionName, args, resultType } =>
      executeCall(libraryPath, functionName, args, resultType, cache),

    FFIRequest::Alloc { length } => unsafe {
      let ptr: *mut c_void = libc::malloc(length);
      if ptr.is_null() { return Err(FFIError::Other("malloc returned null".to_string())); }
      Ok(Value::Pointer(ptr as usize))
    },

    FFIRequest::Free { pointer } => unsafe {
      libc::free(pointer as *mut c_void);
      Ok(Value::None)
    }

    FFIRequest::ReadMemory { pointer, length } => unsafe {
      if pointer == 0 { return Err(FFIError::BadArgument("null pointer".to_string())); }
      let slice: &[u8] = std::slice::from_raw_parts(pointer as *const u8, length);
      Ok(Value::RawString(slice.to_vec()))
    },

    FFIRequest::WriteMemory { pointer, value } => unsafe {
      if pointer == 0 { return Err(FFIError::BadArgument("null pointer".to_string())); }
      let bytes: &[u8] = match &value {
        Value::RawString(v) | Value::CString(v) => v.as_slice(),
        _ => return Err(FFIError::BadArgument("expected RawString or CString for WriteMemory".to_string())),
      };
      std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer as *mut u8, bytes.len());
      Ok(Value::None)
    }
  }
}

/// Executes inside the forked zygote worker, not the Zygote itself;
/// 
/// performs `dlopen` of a specific library and calls the function through libffi;
/// 
/// is called once per request, after which the worker terminates.
fn executeCall(
  libraryPath: String,
  functionName: String,
  args: Vec<Value>,
  ffiResultType: Type,
  cache: &mut FxHashMap<String, Library>
) -> Result<Value, FFIError>
{
  // Check arguments for the presence of Value::None before building C ABI types
  for (index, arg) in args.iter().enumerate() {
    if matches!(arg, Value::None) {
      return Err(FFIError::BadArgument(format!("Cannot pass Value::None as argument at index {}", index)));
    }
  }

  // This code is executed in a clone of the main zygote;
  // All resources will be automatically released when the process terminates.

  // Retrieve the library from the cache or load it from disk on the first call
  if !cache.contains_key(&libraryPath) {
    let lib: Library = unsafe {
      Library::new(&libraryPath)
        .map_err(|e| FFIError::LibraryLoadFailed { libraryPath: libraryPath.clone(), message: e.to_string() })?
    };
    cache.insert(libraryPath.clone(), lib);
  }
  let library: &Library = cache.get(&libraryPath).unwrap();

  // Get the function pointer
  let functionPointer: *mut c_void = unsafe {
    *library
      .get::<*mut c_void>(functionName.as_bytes())
      .map_err(|_| FFIError::SymbolNotFound { functionName: functionName.clone() })?
  };

  // Build argument types for CIF
  let mut argsTypes: Vec<libffi::middle::Type> = Vec::new();
  for arg in &args {
    argsTypes.extend(toCifTypes(arg)?);
  }

  let returnType: libffi::middle::Type = libffi::middle::Type::from(&ffiResultType);

  let cif: Cif = Cif::new(argsTypes, returnType);

  // Prepare storage for the values that the arguments will reference
  let mut storage: Vec<Box<dyn Any>> = Vec::with_capacity(args.len());

  // Build the list of arguments for libffi
  let argsFfi: Vec<Arg> = prepareFFIArgs(&args, &mut storage)?;

  // Function call
  let codePointer: CodePtr = CodePtr(functionPointer);
  let ffiResult: Value = invokeFFI(&cif, codePointer, &argsFfi, &ffiResultType);

  //
  Ok(ffiResult)
}

// =================================================================================================