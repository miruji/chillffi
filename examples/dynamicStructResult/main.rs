use chillffi::ffi;
use chillffi::ffi::scope::Scope;
use chillffi::ffi::types::primitive::{DynamicList, Pointer};
use chillffi::ffi::types::Type;
// =================================================================================================

/// Example of using a dynamically described C struct as an FFI call result.
///
/// The example covers a function returning `struct Data *`, where the struct
/// contains both a regular field and a pointer field:
///
/// ```c
/// struct Data { int size; int *values; };
/// struct Data *process();
/// ```
///
/// The returned struct is read using its runtime `Vec<Type>` layout, while
/// the pointed-to data is read separately using the size stored in the
/// struct. This demonstrates how pointer-returned structs and pointer fields
/// can be handled without special struct-return support.
fn main() -> ()
{
  let values: Vec<i32> = ffi!(|scope| {
    scope.addSearchPath("examples/dynamicStructResult");
    let lib: Library = scope.load("libdata.so")?;

    // struct Data *process();
    let dataPtr: Pointer = lib.call("process").result()?;

    // Dereference the returned pointer using the known struct layout;
    // struct Data { int size; int *values; }
    let fields: DynamicList = Scope::readDynamicStruct(dataPtr, &vec![Type::I32, Type::Pointer])?;
    let size: i32 = fields.get(0)?;
    let valuesPtr: Pointer = fields.get(1)?;

    // `values` is itself a pointer — `size` (its sibling field) is the only
    // place its length lives, so it's a manual readMemory + reinterpret,
    // same as it would be in any other C FFI binding.
    let bytes: Vec<u8> = Scope::readMemory(valuesPtr, size as usize * 4)?;
    let values: Vec<i32> = bytes
      .chunks_exact(4)
      .map(|c| i32::from_ne_bytes(c.try_into().expect("4-byte chunk")))
      .collect();

    // process() malloc'd both the struct and its values array — free both
    // through the C side's own freeData(), not field-by-field via
    // Scope::free(), since only the C side actually knows the true
    // allocation shape (and here, that values itself was a separate malloc).
    lib.call("freeData").arg(dataPtr).void()?;

    Ok(values)
  }).expect("dynamic struct result failed");

  //
  println!("process() -> Data {{ size: {}, values: {:?} }}", values.len(), values);
  assert_eq!(values, vec![10, 20, 30]);
  println!("ok: dynamic struct as a call result");
}

// =================================================================================================