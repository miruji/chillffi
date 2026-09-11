mod platform;
use crate::platform::LibcPath;
// =================================================================================================
use chillffi::ffi;
use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::errors::FFIError;
// =================================================================================================

/// Print custom message via libc's puts
fn println(text: &str) -> Result<(), FFIError>
{
  ffi!(|scope| {
    let libc: Library = scope.load(LibcPath)?;

    // C-string null termination
    let mut bytes: Vec<u8> = text.as_bytes().to_vec();
    bytes.push(0);

    // Allocate memory and write null-terminated string bytes
    let mem: AllocatedMemory = scope.alloc(bytes.len())?;
    mem.write(bytes)?;

    // puts(const char *s) automatically appends a newline
    libc.call("puts")
      .arg(mem.asPointer())
      .void()?;
    
    //
    Ok(())
  })
}

fn main() -> ()
{
  println("Hello from libc via chillffi!")
    .expect("Failed to print via libc");
}

// =================================================================================================