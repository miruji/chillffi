use std::cmp::Ordering;
use chillffi::ffi::value::{Value, Type};
use chillffi::ffi;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::callback;
// =================================================================================================

/// Demonstrates passing a Rust closure as a C function pointer to `qsort` via FFI.
fn main() -> ()
{
  println!("=== Starting qsort via chillffi ===\n");

  let sorted: Vec<i32> = ffi!(|scope| {
    let libc: Library = scope.load("libc.so.6")?;
    println!("[ffi!] Loaded libc.so.6");
    
    // Allocate memory inside the clone for the array.
    let mem: AllocatedMemory = scope.alloc(5 * 4)?;
    let ptrAddr: usize = match mem.asPointer() {
      Value::Pointer(addr) => addr,
      other => panic!("expected Pointer, got {:?}", other),
    };
    println!("[ffi!] Allocated 20 bytes at address: 0x{:X}", ptrAddr);
    
    // Initialize the source data.
    let data: [i32; 5] = [3, 1, 4, 1, 5];
    println!("[ffi!] Original data: {:?}", data);
    
    // Write the source data into the allocated clone memory.
    let raw: &[u8] = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, 20) };
    mem.write(Value::RawString(raw.to_vec()))?;
    println!("[ffi!] Written raw bytes to clone memory\n");

    // Empty capture list — the comparator takes nothing from the outer scope,
    // only its `args` parameter. If a capture was needed (e.g.,
    // `threshold`), the list would look like `callback!([threshold: i32] |args| ...)`.
    let compar = callback!([] |args: Vec<Value>| -> Value 
    {
      let a: usize = match args[0] {
          Value::Pointer(a) => a,
          _ => panic!("expected Pointer for arg 0"),
      };
      let b: usize = match args[1] {
          Value::Pointer(b) => b,
          _ => panic!("expected Pointer for arg 1"),
      };
      
      // Direct dereferencing is correct: the closure runs inside the clone
      // (where the data resides), not in the parent process.
      let av: i32 = unsafe { *(a as *const i32) };
      let bv: i32 = unsafe { *(b as *const i32) };

      let cmp: Ordering = av.cmp(&bv);
      let result: i32 = cmp as i32;

      //
      println!(
        "  [callback] comparing *0x{:X} = {}  vs  *0x{:X} = {}  =>  {}",
        a, av, b, bv,
        match cmp {
          Ordering::Less    => "LESS (return -1)",
          Ordering::Equal   => "EQUAL (return 0)",
          Ordering::Greater => "GREATER (return 1)",
        }
      );

      Value::I32(result)
    });
    
    // Register the closure in the clone's callback registry.
    let compar: Value = scope.callback(
      vec![Type::Pointer, Type::Pointer],
      Type::I32,
      compar,
    );
    println!("[ffi!] Registered comparator callback\n");
    
    // Execute the C function.
    println!("[ffi!] Calling qsort(mem, 5, 4, compar)...");
    libc.call("qsort")
      .arg(mem.asPointer())
      .arg::<usize>(5)
      .arg::<usize>(4)
      .arg(compar)
      .void()?;
    println!("[ffi!] qsort returned\n");
    
    // Read the sorted memory block back into the parent process.
    let Value::RawString(bytes) = mem.read()? else { panic!() };
    println!("[ffi!] Read back raw bytes: {:?}\n", bytes);
    
    // Reconstruct the Rust vector from the raw bytes.
    let vec: Vec<i32> = bytes.chunks_exact(4)
      .map(|b| i32::from_ne_bytes(b.try_into().unwrap()))
      .collect();

    println!("[ffi!] Decoded to i32 vector: {:?}", vec);

    Ok(vec)
  }).expect("qsort failed");

  //
  println!("\n=== Result outside ffi! block ===");
  println!("sorted = {:?}", sorted);
  assert_eq!(sorted, vec![1, 1, 3, 4, 5]);
  println!("Assertion passed: [1, 1, 3, 4, 5] ✓");
}