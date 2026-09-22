#include <stdlib.h>

#ifdef _WIN32
  #define ChillffiExport __declspec(dllexport)
#else
  #define ChillffiExport
#endif

// Deliberately crashes the process it runs in. Used to prove the
// isolation boundary holds: nothing on the Rust side is supposed to
// survive *inside* a crashed clone, but the parent process — and every
// FFI call made after this one — must be completely unaffected.

ChillffiExport void triggerSegfault(void)
{
  volatile int *p = (int *)0;
  *p = 1;
}

ChillffiExport void triggerAbort(void)
{
  abort();
}
