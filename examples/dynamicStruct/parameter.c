#include <stdlib.h>

// A C function that takes a
// pointer to a struct built by the caller.

struct Data {
  int size;
  int *values;
};

static int lastSum = 0;

void process(struct Data *data) {
  int sum = 0;
  for (int i = 0; i < data->size; i++) {
    sum += data->values[i];
  }
  lastSum = sum;
}

int getSum(void) {
  return lastSum;
}
