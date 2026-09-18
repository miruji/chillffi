#include <stdlib.h>

#ifdef _WIN32
  #define CHILLFFI_EXPORT __declspec(dllexport)
#else
  #define CHILLFFI_EXPORT
#endif

// A C function that takes a
// pointer to a struct built by the caller.

struct Data {
  int size;
  int *values;
};

static int lastSum = 0;

CHILLFFI_EXPORT void process(struct Data *data) {
  int sum = 0;
  for (int i = 0; i < data->size; i++) {
    sum += data->values[i];
  }
  lastSum = sum;
}

CHILLFFI_EXPORT int getSum(void) {
  return lastSum;
}
