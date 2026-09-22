#include <stdint.h>

#ifdef _WIN32
  #define ChillffiExport __declspec(dllexport)
#else
  #define ChillffiExport
#endif

// Small struct: typically passed in registers on x86_64 SysV / aarch64.
struct Point {
  int32_t x;
  int32_t y;
};

// Larger than 16 bytes on x86_64 SysV: passed via memory / the stack.
struct Big {
  double a;
  double b;
  double c;
  int32_t tag;
};

// Nested: outer holds a Point by value.
struct Nested {
  struct Point origin;
  float scale;
};

ChillffiExport int32_t pointSum(struct Point p)
{
  return p.x + p.y;
}

ChillffiExport struct Point pointTranslate(struct Point p, int32_t dx, int32_t dy)
{
  struct Point out;
  out.x = p.x + dx;
  out.y = p.y + dy;
  return out;
}

ChillffiExport double bigSum(struct Big b)
{
  return b.a + b.b + b.c + (double)b.tag;
}

ChillffiExport struct Big bigScale(struct Big b, double k)
{
  struct Big out;
  out.a = b.a * k;
  out.b = b.b * k;
  out.c = b.c * k;
  out.tag = b.tag;
  return out;
}

ChillffiExport float nestedScale(struct Nested n)
{
  return n.scale * (float)(n.origin.x + n.origin.y);
}

ChillffiExport struct Nested nestedDouble(struct Nested n)
{
  struct Nested out;
  out.origin.x = n.origin.x * 2;
  out.origin.y = n.origin.y * 2;
  out.scale = n.scale * 2.0f;
  return out;
}
