#include <stdlib.h>

// Deliberately crashes the process it runs in. Used to prove the
// isolation boundary holds: nothing on the Rust side is supposed to
// survive *inside* a crashed clone, but the parent process — and every
// FFI call made after this one — must be completely unaffected.

void triggerSegfault(void)
{
  volatile int *p = (int *)0;
  *p = 1;
}

void triggerAbort(void)
{
  abort();
}
