#include <errno.h>

// Sets errno to a caller-chosen value and returns -1, mirroring a C
// function that reports failure only through errno (e.g. strtol, the
// malloc family, open) rather than through its return value.
int failWithErrno(int code)
{
  errno = code;
  return -1;
}