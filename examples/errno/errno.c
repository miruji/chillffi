#include <errno.h>

#ifdef _WIN32
  #define ChillffiExport __declspec(dllexport)
  #include <windows.h>
#else
  #define ChillffiExport
#endif

// Sets errno to a caller-chosen value and returns -1, mirroring a C
// function that reports failure only through errno (e.g. strtol, the
// malloc family, open) rather than through its return value.
ChillffiExport int failWithErrno(int code)
{
  errno = code;
  return -1;
}

#ifdef _WIN32
// Sets the Win32 last-error code (GetLastError channel) and returns -1.
// Most Win32 APIs report failure here, not through CRT errno. Captured by
// chillffi under the same `.errno()` / setReadErrno flag as lastErrno,
// and read back via Scope::lastOsError().
ChillffiExport int failWithOsError(unsigned long code)
{
  SetLastError(code);
  return -1;
}
#endif
