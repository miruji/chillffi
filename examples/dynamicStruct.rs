use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::value::{Type, Value};
use chillffi::ffi::errors::FFIError;
use chillffi::ffi::scope::Scope;
use chillffi::ffi;
// =================================================================================================

/// Call clock_gettime and extract fields using dynamic struct layouts.
fn main() -> ()
{
  // Define dynamic struct shape for timespec (time_t tv_sec; long tv_nsec;)
  let timespecShape: Vec<Type> = vec![Type::I64, Type::I64];

  let (secs, nanos): (i64, i64) = ffi!(|scope| {
    let libc: Library = Library::load("libc.so.6")?;
    
    // Allocate memory for timespec struct (16 bytes)
    let mem: AllocatedMemory = scope.alloc(16)?;

    // Call clock_gettime(CLOCK_REALTIME, mem)
    libc.call("clock_gettime")
      .arg::<i32>(0)
      .arg(mem.asPointer())
      .void()?;

    // Read dynamically shaped struct from memory using libffi ABI rules
    let fields: Vec<Value> = Scope::readDynamicStruct(mem.address(), &timespecShape)?;

    // Parse extracted fields
    let [Value::I64(secs), Value::I64(nanos)] = fields.as_slice() else {
      return Err(FFIError::Other("expected two I64 fields".into()));
    };

    Ok((*secs, *nanos))
  }).expect("readDynamicStruct failed");

  // Define a nested dynamic struct shape
  let nestedShape: Vec<Type> = vec![
    Type::Struct(vec![Type::U64, Type::U8, Type::F64]),
    Type::I8,
    Type::I64,
  ];

  //
  println!("clock_gettime: {secs}.{nanos:09}");
  println!("nested shape root fields: {}", nestedShape.len());

  assert_eq!(nestedShape.len(), 3);
  println!("ok: dynamic struct layouts");
}

// =================================================================================================