use chillffi::ffi::allocatedMemory::AllocatedMemory;
use chillffi::ffi::value::{Value};
use chillffi::ffi::errors::FFIError;
use chillffi::ffi;
// =================================================================================================

/// Print custom message via libc's puts
fn println(text: &str) -> Result<(), FFIError>
{
  ffi!(|scope| {
    let libc: Library = Library::load("libc.so.6")?;

    // C-string null termination
    let mut bytes: Vec<u8> = text.as_bytes().to_vec();
    bytes.push(0);

    // Allocate memory and write null-terminated string bytes
    let mem: AllocatedMemory = scope.alloc(bytes.len())?;
    mem.write(Value::RawString(bytes))?;

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