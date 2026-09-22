#include <stdlib.h>

#ifdef _WIN32
  #define ChillffiExport __declspec(dllexport)
#else
  #define ChillffiExport
#endif

// A C function that takes a
// pointer to a struct built by the caller.

struct Data {
  int size;
  int *values;
};

static int lastSum = 0;

ChillffiExport void process(struct Data *data) {
  int sum = 0;
  for (int i = 0; i < data->size; i++) {
    sum += data->values[i];
  }
  lastSum = sum;
}

ChillffiExport int getSum(void) {
  return lastSum;
}
