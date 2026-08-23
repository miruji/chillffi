#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

// Accepts raw argument bytes (concatenated sequentially),
// prints them as a string and returns NULL.
uint8_t* print(const uint8_t* data, size_t len) .
{
  fwrite(data, 1, len, stdout);
  fflush(stdout);
  return NULL;
}