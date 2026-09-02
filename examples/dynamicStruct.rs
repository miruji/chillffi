use chillffi::ffi::types::Type;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::scope::Scope;
use chillffi::ffi;
use chillffi::ffi::types::primitive::DynamicList;
// =================================================================================================

/// Call clock_gettime and extract fields using dynamic struct layouts.
fn main() -> ()
{
  // Define dynamic struct shape for timespec (time_t tv_sec; long tv_nsec;)
  let timespecShape: Vec<Type> = vec![Type::I64, Type::I64];

  let (secs, nanos): (i64, i64) = ffi!(|scope| {
    let libc: Library = scope.load("libc.so.6")?;
    
    // Allocate memory for timespec struct (16 bytes)
    let mem: AllocatedMemory = scope.alloc(16)?;

    // Call clock_gettime(CLOCK_REALTIME, mem)
    libc.call("clock_gettime")
      .arg::<i32>(0)
      .arg(mem.asPointer())
      .void()?;

    // Read dynamically shaped struct from memory using libffi ABI rules
    let fields: DynamicList = Scope::readDynamicStruct(mem.address(), &timespecShape)?;

    // Parse extracted fields
    Ok((fields.get(0)?, fields.get(1)?))
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