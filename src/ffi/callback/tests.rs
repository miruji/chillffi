
// =================================================================================================

#[cfg(test)]
mod tests
{
  use crate::ffi::callback::Envelope;
  use crate::ffi::callback::CallError;
  use crate::ffi::callback::decode;
  use crate::ffi::callback::Callable;
  // ===============================================================================================

  /// Round-trips a closure through encode/decode within a single process.
  #[test]
  fn roundtrip() -> ()
  {
    let threshold: i32 = 5;
    let compar: Callback = callback!([threshold: i32] |args: Vec<i32>| -> i32 {
      args.iter().filter(|&&x| x > threshold).count() as i32
    });

    assert_eq!(compar.call(vec![1, 6, 9, 2]), 2);

    let bytes: Vec<u8> = compar.encode().expect("encode");
    let remote: Box<dyn Callable<Vec<i32>, i32>> = decode(&bytes).expect("decode");
    assert_eq!(remote.call(vec![1, 6, 9, 2]), 2);
  }

  /// Requesting the wrong Args/Output must fail cleanly — checked by the
  /// caller before any pointer resolution happens.
  #[test]
  fn argsOutputMismatchIsCaught() -> ()
  {
    let x: i32 = 1;
    let c: Callback = callback!([x: i32] |args: Vec<i32>| -> i32 { args.len() as i32 + x });
    let bytes: Vec<u8> = c.encode().expect("encode");

    let wrong: Result<Box<dyn Callable<String, bool>>, CallError> = decode(&bytes);
    assert!( matches!(wrong, Err(CallError::ArgsOutputMismatch)) );
  }

  /// A resolved-but-wrong site (simulated by hand-corrupting the tag) must
  /// be caught by the target function itself, not silently produce garbage.
  #[test]
  fn siteTagMismatchIsCaught() -> ()
  {
    let x: i32 = 1;
    let c: Callback = callback!([x: i32] |args: Vec<i32>| -> i32 { args.len() as i32 + x });
    let mut bytes: Vec<u8> = c.encode().expect("encode");

    let (mut envelope, _): (Envelope, usize) = 
      bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
    envelope.siteTag = envelope.siteTag.wrapping_add(1);
    bytes = bincode::serde::encode_to_vec(&envelope, bincode::config::standard()).unwrap();

    let result: Result<Box<dyn Callable<Vec<i32>, i32>>, CallError> = decode(&bytes);
    assert!( matches!(result, Err(CallError::TypeMismatch { .. })) );
  }

  // ===============================================================================================
}

// =================================================================================================