#include <stdlib.h>

// A C function returning a pointer to a
// struct it allocated dynamically on the heap.

struct Data {
  int size;
  int *values;
};

struct Data *process(void) {
  struct Data *data = malloc(sizeof(struct Data));
  data->size = 3;
  data->values = malloc(sizeof(int) * data->size);
  data->values[0] = 10;
  data->values[1] = 20;
  data->values[2] = 30;
  return data;
}

void freeData(struct Data *data) {
  if (!data) return;
  free(data->values);
  free(data);
}
