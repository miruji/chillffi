#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
  #define ChillffiExport __declspec(dllexport)
#else
  #define ChillffiExport
#endif

// C owns the full allocation lifecycle. The buffer never escapes to the
// caller — Rust receives only a computed result and must not call free /
// Scope::free on anything from these functions.

// Allocates, fills, sums, frees. Returns the sum. No pointer leaves C.
ChillffiExport int processOwned(void)
{
  int *values = (int *)malloc(sizeof(int) * 3);
  if (!values) return -1;

  values[0] = 10;
  values[1] = 20;
  values[2] = 30;

  int sum = values[0] + values[1] + values[2];
  free(values);
  return sum;
}

// Takes a Rust/IPC string, makes an internal copy, measures it, frees the
// copy before return. Again nothing for the caller to release.
ChillffiExport int measureOwnedCopy(const char *s)
{
  if (!s) return -1;

  char *copy = (char *)malloc(strlen(s) + 1);
  if (!copy) return -1;

  memcpy(copy, s, strlen(s) + 1);
  int n = (int)strlen(copy);
  free(copy);
  return n;
}
