#include <stdlib.h>

#ifdef _WIN32
  #define CHILLFFI_EXPORT __declspec(dllexport)
#else
  #define CHILLFFI_EXPORT
#endif

// Deliberately crashes the process it runs in. Used to prove the
// isolation boundary holds: nothing on the Rust side is supposed to
// survive *inside* a crashed clone, but the parent process — and every
// FFI call made after this one — must be completely unaffected.

CHILLFFI_EXPORT void triggerSegfault(void)
{
  volatile int *p = (int *)0;
  *p = 1;
}

CHILLFFI_EXPORT void triggerAbort(void)
{
  abort();
}
