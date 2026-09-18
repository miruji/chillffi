#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
  #define CHILLFFI_EXPORT __declspec(dllexport)
#else
  #define CHILLFFI_EXPORT
#endif

// Accepts raw argument bytes (concatenated sequentially),
// prints them as a string and returns NULL.
CHILLFFI_EXPORT uint8_t* print(const uint8_t* data, size_t len)
{
  fwrite(data, 1, len, stdout);
  fflush(stdout);
  return NULL;
}