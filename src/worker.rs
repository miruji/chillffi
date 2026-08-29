use crate::ffi::callback;
use parking_lot::{Mutex, RawMutex};
use std::sync::OnceLock;
use libffi::middle::Closure;
use crate::ffi::errors::FFIError;
use std::any::Any;
use libloading::Library;
use libffi::middle::{Arg, Cif, CodePtr};
use std::ffi::c_void;
use fxhash::FxHashMap;
use parking_lot::lock_api::MutexGuard;
use crate::ffi::value::{Type, Value};
use crate::zygote::{FFIRequest};
use crate::ffi::callback::Callable;
// =================================================================================================

/// Callback registry inside the clone (not parent)
struct CallbackWrapper
{
  /// The decoded Rust closure to be invoked.
  closure: Box<dyn Callable<Vec<Value>, Value>>,
  
  /// Expected FFI argument types for correct deserialization.
  argTypes: Vec<Type>,
  
  /// Expected FFI return type.
  returnType: Box<Type>
}

/// Holds the JIT-compiled closure and its raw function pointer for FFI execution.
struct CallbackEntry
{
  /// Never read directly — kept alive purely for its `Drop` impl. `Closure`
  /// owns the JIT-compiled trampoline memory that `codePointer` points into;
  /// 
  /// if this field were removed (or dropped early), `codePointer` would
  /// dangle the moment registration returns, and C would jump into freed
  /// memory on the very next call. This is intentional RAII, not dead code.
  #[allow(dead_code)]
  closure: Closure<'static>,
  
  /// Raw C function pointer to the JIT-compiled trampoline.
  codePointer: *mut c_void
}

/// Safety: CallbackRegistry is used exclusively within a single fork-clone,
/// which operates as a single-threaded process.
unsafe impl Send for CallbackEntry {}

/// Global map storing registered JIT-compiled callbacks by their unique IDs.
static CallbackRegistry: OnceLock<Mutex<FxHashMap<u64, CallbackEntry>>> = OnceLock::new();

thread_local!{
  /// Set by `trampoline` if the user's closure panics mid-call.
  ///
  /// Panics cannot propagate through C stack frames. `catch_unwind` traps it, 
  /// and this flag becomes the only way to surface the error. 
  ///
  /// `executeCall` checks this after the C call returns, producing a clean 
  /// `FFIError` instead of silently returning a default/zero value.
  static CallbackPanicked: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Initializes and returns a reference to the global callback registry.
fn registry() -> &'static Mutex<FxHashMap<u64, CallbackEntry>> {
  CallbackRegistry.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// The universal C-callable trampoline. 
/// 
/// Converts C arguments to Rust `Value`s, invokes the closure, and translates the result back.
unsafe extern "C" fn trampoline(
  _cif: &libffi::low::ffi_cif,
  ret: &mut std::ffi::c_void,
  args: *const *const std::ffi::c_void,
  userdata: &CallbackWrapper,
)
{
  // Safely construct a slice of raw C argument pointers based on the expected length.
  let cArgs: &[*const c_void] = unsafe { std::slice::from_raw_parts(args, userdata.argTypes.len()) };
  let mut rustArgs: Vec<Value> = Vec::with_capacity(userdata.argTypes.len());

  // Convert each raw C argument into a safe Rust `Value` according to its expected signature.
  for (i, typ) in userdata.argTypes.iter().enumerate() {
    rustArgs.push(readArg(cArgs[i], typ));
  }

  // Invoke the Rust closure while catching panics to prevent unwinding across the C boundary.
  //
  // On panic, we signal the thread-local flag and return a dummy `None` value 
  // so C code can gracefully resume (and eventually return control to our safe wrapper).
  let result: Value =
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| userdata.closure.call(rustArgs)))
      .unwrap_or_else(|_| {
        CallbackPanicked.with(|f| f.set(true));
        Value::None
      });
  writeRet(ret, result, &userdata.returnType);
}

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
    Value::Function(_) => Ok(vec![libffi::middle::Type::pointer()]),
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
  storage: &'a mut Vec<Box<dyn Any>>
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
      Value::Function(id) => {
        let codePointer: *mut c_void = {
          let reg: MutexGuard<RawMutex, FxHashMap<u64, CallbackEntry>> = registry().lock();
          let entry: &CallbackEntry = reg
            .get(id)
            .ok_or_else(|| FFIError::Other(format!("callback {} not registered", id)))?;
          entry.codePointer
        };
        storage.push(Box::new(codePointer));
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
      Value::Function { .. } => {
        let ptr: &*mut c_void = downcastRef(&storage[i])?;
        argsFfi.push(Arg::new(ptr));
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
      let ptr: *mut c_void = unsafe{ cif.call::<*mut c_void>(codePointer, argsFfi) };
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

/// Constructs a libffi `Cif` (Call Interface) defining the argument and return types.
fn buildCif(argTypes: &[Type], returnType: &Type) -> Result<libffi::middle::Cif, FFIError>
{
  let mut argsTypes: Vec<libffi::middle::Type> = Vec::with_capacity(argTypes.len());
  for t in argTypes {
    argsTypes.push(libffi::middle::Type::from(t));
  }
  let returnType: libffi::middle::Type = libffi::middle::Type::from(returnType);
  Ok(libffi::middle::Cif::new(argsTypes, returnType))
}

/// Safely reads a raw C pointer and converts it into a Rust `Value` based on the specified `Type`.
const fn readArg(ptr: *const std::ffi::c_void, t: &Type) -> Value
{
  match t 
  {
    Type::None => Value::None,
    Type::U8 => Value::U8(unsafe { *(ptr as *const u8) }),
    Type::U16 => Value::U16(unsafe { *(ptr as *const u16) }),
    Type::U32 => Value::U32(unsafe { *(ptr as *const u32) }),
    Type::U64 => Value::U64(unsafe { *(ptr as *const u64) }),
    Type::Usize => Value::Usize(unsafe { *(ptr as *const usize) }),
    Type::I8 => Value::I8(unsafe { *(ptr as *const i8) }),
    Type::I16 => Value::I16(unsafe { *(ptr as *const i16) }),
    Type::I32 => Value::I32(unsafe { *(ptr as *const i32) }),
    Type::I64 => Value::I64(unsafe { *(ptr as *const i64) }),
    Type::Isize => Value::Isize(unsafe { *(ptr as *const isize) }),
    Type::F32 => Value::F32(unsafe { *(ptr as *const f32) }),
    Type::F64 => Value::F64(unsafe { *(ptr as *const f64) }),
    Type::Bool => Value::Bool(unsafe { *(ptr as *const u8) != 0 }),
    Type::Pointer => Value::Pointer(unsafe { *(ptr as *const usize) }),
  }
}

/// Writes a Rust `Value` back into the raw C return pointer based on the expected `Type`.
fn writeRet(ret: &mut std::ffi::c_void, value: Value, t: &Type)
{
  match (t, value) 
  {
    (Type::None, _) => {}
    (Type::U8,  Value::U8(v))  => unsafe { *(ret as *mut std::ffi::c_void as *mut u8)  = v },
    (Type::U16, Value::U16(v)) => unsafe { *(ret as *mut std::ffi::c_void as *mut u16) = v },
    (Type::U32, Value::U32(v)) => unsafe { *(ret as *mut std::ffi::c_void as *mut u32) = v },
    (Type::U64, Value::U64(v)) => unsafe { *(ret as *mut std::ffi::c_void as *mut u64) = v },
    (Type::Usize, Value::Usize(v)) => unsafe { *(ret as *mut std::ffi::c_void as *mut usize) = v },
    (Type::I8,  Value::I8(v))  => unsafe { *(ret as *mut std::ffi::c_void as *mut i8)  = v },
    (Type::I16, Value::I16(v)) => unsafe { *(ret as *mut std::ffi::c_void as *mut i16) = v },
    (Type::I32, Value::I32(v)) => unsafe { *(ret as *mut std::ffi::c_void as *mut i32) = v },
    (Type::I64, Value::I64(v)) => unsafe { *(ret as *mut std::ffi::c_void as *mut i64) = v },
    (Type::Isize, Value::Isize(v)) => unsafe { *(ret as *mut std::ffi::c_void as *mut isize) = v },
    (Type::F32, Value::F32(v)) => unsafe { *(ret as *mut std::ffi::c_void as *mut f32) = v },
    (Type::F64, Value::F64(v)) => unsafe { *(ret as *mut std::ffi::c_void as *mut f64) = v },
    (Type::Bool, Value::Bool(v)) => unsafe { *(ret as *mut std::ffi::c_void as *mut u8) = if v { 1 } else { 0 } },
    (Type::Pointer, Value::Pointer(v)) => unsafe { *(ret as *mut std::ffi::c_void as *mut usize) = v },
    _ => {}
  }
}

// =================================================================================================

/// Dispatches an FFI request inside the zygote clone to the appropriate handler.
pub(super) fn executeFFI(
  request: FFIRequest,
  cache: &mut FxHashMap<String, Library>
) -> Result<Value, FFIError>
{
  match request
  {
    FFIRequest::Call { libraryPath, functionName, args, resultType } =>
      executeCall(libraryPath, functionName, args, resultType, cache),

    FFIRequest::CallPointer { pointer, args, resultType } =>
      executeCallPointer(pointer, args, resultType),

    FFIRequest::Alloc { length } => {
      let ptr: *mut c_void = unsafe{ libc::malloc(length) };
      if ptr.is_null() { return Err(FFIError::Other("malloc returned null".to_string())); }
      Ok(Value::Pointer(ptr as usize))
    },

    FFIRequest::Free { pointer } => {
      unsafe{ libc::free(pointer as *mut c_void) };
      Ok(Value::None)
    }

    FFIRequest::ReadMemory { pointer, length } => {
      if pointer == 0 { return Err(FFIError::BadArgument("null pointer".to_string())); }
      let slice: &[u8] = unsafe{ std::slice::from_raw_parts(pointer as *const u8, length) };
      Ok(Value::RawString(slice.to_vec()))
    },

    FFIRequest::WriteMemory { pointer, value } => {
      if pointer == 0 { return Err(FFIError::BadArgument("null pointer".to_string())); }
      let bytes: &[u8] = match &value {
        Value::RawString(v) | Value::CString(v) => v.as_slice(),
        _ => return Err(FFIError::BadArgument("expected RawString or CString for WriteMemory".to_string())),
      };
      unsafe{ std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer as *mut u8, bytes.len()) };
      Ok(Value::None)
    }

    FFIRequest::RegisterCallback { id, bytes, argTypes, returnType } => {
      let wrapper: Box<dyn Callable<Vec<Value>, Value>> = callback::decode(&bytes)
        .map_err(|e| FFIError::Other(format!("call decode failed: {e}")))?;
      let cif: Cif = buildCif(&argTypes, &returnType)?;
      let leaked: &mut CallbackWrapper = Box::leak(Box::new(CallbackWrapper {
        closure: wrapper,
        argTypes,
        returnType: Box::new(returnType)
      }));
      let closure: Closure = Closure::new(cif, trampoline, leaked);
      let codeAddr: usize = *closure.code_ptr() as usize;
      let codePointer: *mut c_void = codeAddr as *mut c_void;
      registry().lock().insert(id, CallbackEntry { closure, codePointer });
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

  invokeAtPointer(functionPointer as usize, args, ffiResultType)
}

/// Calls a raw function pointer directly — no `dlopen`/`dlsym`, the address
/// is already known (typically a pointer previously *returned* by another
/// FFI call, e.g. `signal()`'s return value, or a pointer read out of a
/// dispatch table). Only meaningful within the same clone the pointer came
/// from — same rules as `Value::Pointer`'s own lifetime.
fn executeCallPointer(
  pointer: usize,
  args: Vec<Value>,
  ffiResultType: Type
) -> Result<Value, FFIError>
{
  if pointer == 0 {
    return Err(FFIError::BadArgument("null function pointer".to_string()));
  }

  invokeAtPointer(pointer, args, ffiResultType)
}

/// Shared by `executeCall` (address resolved via `dlsym`) and
/// `executeCallPointer` (a raw address handed to us directly) — everything
/// past "we have a code address" is identical either way.
fn invokeAtPointer(
  pointer: usize,
  args: Vec<Value>,
  ffiResultType: Type
) -> Result<Value, FFIError>
{
  // Check arguments for the presence of Value::None before building C ABI types
  for (index, arg) in args.iter().enumerate() {
    if matches!(arg, Value::None) {
      return Err(FFIError::BadArgument(format!("Cannot pass Value::None as argument at index {}", index)));
    }
  }

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
  let codePointer: CodePtr = CodePtr(pointer as *mut c_void);
  let ffiResult: Value = invokeFFI(&cif, codePointer, &argsFfi, &ffiResultType);

  // A registered callback may have panicked mid-call (see `trampoline` +
  // `CallbackPanicked`) — the C function still "completed" and ffiResult is
  // whatever came out of that, but at least one comparison/predicate/etc.
  // ran on a default value instead of the real one. Don't hand back a
  // result the caller would trust as correct.
  if CallbackPanicked.with(|f| f.replace(false)) {
    return Err(FFIError::Other("a registered callback panicked during the call".to_string()));
  }

  Ok(ffiResult)
}

// =================================================================================================