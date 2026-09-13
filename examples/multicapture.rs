mod platform;
use crate::platform::LibcPath;
// =================================================================================================
use chillffi::callback;
use chillffi::ffi;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::types::primitive::Callback;
use chillffi::ffi::types::primitive::Pointer;
use std::cmp::Ordering;
// =================================================================================================

/// Demonstrates capturing *several* variables of *different* types (`i32`
/// and `bool`) into one `callback!` closure with no explicit `[]` list —
/// each is captured automatically, exactly like an ordinary Rust closure,
/// and both actually travel into the clone that runs the comparator.
fn main() -> ()
{
  println!("=== Sorting with a multi-capture comparator ===\n");

  // Two unrelated captured variables — no capture list needed for either.
  let bias: i32 = 3;           // shifts every value before comparing
  let descending: bool = true; // flips the resulting order

  let sorted: Vec<i32> = ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;
    println!("[ffi!] Loaded {}", LibcPath);

    // Allocate memory inside the clone for the array.
    let mem: AllocatedMemory = scope.alloc(5 * 4)?;
    println!("[ffi!] Allocated 20 bytes at address: 0x{:X}", mem.asPointer());

    // Initialize the source data.
    let data: [i32; 5] = [3, 1, 4, 1, 5];
    println!("[ffi!] Original data: {:?}", data);

    // Write the source data into the allocated clone memory.
    let raw: &[u8] = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, 20) };
    mem.write(raw)?;
    println!("[ffi!] Written raw bytes to clone memory\n");

    // `bias` and `descending` both come straight from `main` below — no
    // capture list, and the comparator still runs inside the clone (a
    // different process), so this also proves both values were actually
    // serialized across, not just visible by accident.
    let compar: Callback = callback!(scope, |a: Pointer, b: Pointer| -> i32
    {
      let av: i32 = unsafe { *(a.0 as *const i32) };
      let bv: i32 = unsafe { *(b.0 as *const i32) };

      let cmp: Ordering = if descending
      {
        (bv + bias).cmp(&(av + bias))
      }
      else
      {
        (av + bias).cmp(&(bv + bias))
      };

      println!(
        "  [callback] pid={}  comparing {} vs {}  (bias={bias}, descending={descending})  =>  {:?}",
        std::process::id(), av, bv, cmp
      );

      cmp as i32
    });
    println!("[ffi!] Registered comparator capturing `bias` + `descending`\n");

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
    let bytes: Vec<u8> = mem.read()?;

    // Reconstruct the Rust vector from the raw bytes.
    let vec: Vec<i32> = bytes.chunks_exact(4)
      .map(|b| i32::from_ne_bytes(b.try_into().unwrap()))
      .collect();

    println!("[ffi!] Decoded to i32 vector: {:?}", vec);
    Ok(vec)
  }).expect("qsort failed");

  //
  println!("\n=== Result outside ffi! block ===");
  println!("pid = {} (parent)", std::process::id());
  println!("sorted (descending=true) = {:?}", sorted);
  assert_eq!(sorted, vec![5, 4, 3, 1, 1]);
  println!("Assertion passed: [5, 4, 3, 1, 1] ✓");
}

// =================================================================================================