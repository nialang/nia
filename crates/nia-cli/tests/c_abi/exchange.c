/* Freestanding C half of the value-exchange test: every function either
   receives values from Nia and returns derived ones, or calls into Nia and
   checks what comes back. Nothing here needs a C library. */
#include <stdarg.h>

typedef signed char i8;
typedef unsigned char u8;
typedef short i16;
typedef unsigned short u16;
typedef int i32;
typedef long long i64;

struct S3 { u8 a, b, c; };
struct S6 { u16 a, b, c; };
struct F3 { float a, b, c; };
struct D2 { double a, b; };
struct D5 { double a, b, c, d, e; };
struct FD { float a; double b; };
struct IFF { i32 c; float a, b; };
struct P2 { const u8 *a, *b; };
struct Pt { float x, y; };
struct NA { struct Pt p; float z; };
struct BQ { u8 a; i64 b; };
struct W3 { i64 a, b, c; };

struct S3 c_s3(struct S3 v, i32 k) { v.a += k; v.b += k; v.c += k; return v; }
struct S6 c_s6(struct S6 v, i32 k) { v.a += k; v.b += k; v.c += k; return v; }
struct F3 c_f3(struct F3 v, i32 k) { v.a += k; v.b += k; v.c += k; return v; }
struct D2 c_d2(struct D2 v, i32 k) { v.a += k; v.b += k; return v; }
struct D5 c_d5(struct D5 v, i32 k) { v.a += k; v.b += k; v.c += k; v.d += k; v.e += k; return v; }
struct FD c_fd(struct FD v, i32 k) { v.a += k; v.b += k; return v; }
struct IFF c_iff(struct IFF v, i32 k) { v.c += k; v.a += k; v.b += k; return v; }
struct NA c_na(struct NA v, i32 k) { v.p.x += k; v.p.y += k; v.z += k; return v; }
struct BQ c_bq(struct BQ v, i32 k) { v.a += k; v.b += k; return v; }
struct W3 c_w3(struct W3 v, i32 k) { v.a += k; v.b += k; v.c += k; return v; }
struct P2 c_p2(struct P2 v) { struct P2 r = { v.b, v.a }; return r; }
i32 c_ext(i8 a, u8 b, i16 c, u16 d) { return a + b + c + d; }
i8 c_ret_i8(void) { return -3; }
u16 c_ret_u16(void) { return 65535; }
i64 c_many(i64 a, i64 b, i64 c, i64 d, i64 e, struct P2 p, i64 f) {
    return a + b + c + d + e + f + (p.b - p.a);
}
double c_sse(double a, double b, double c, double d, double e, double f, double g,
             struct D2 v, struct FD w, double h) {
    return a + b + c + d + e + f + g + v.a + v.b + w.a + w.b + h;
}
struct W3 c_sret_after(i64 a, i64 b, i64 c, i64 d, i64 e, struct P2 p) {
    struct W3 r = { a + b, c + d, e + (p.b - p.a) };
    return r;
}
double c_var(i32 n, ...) {
    va_list ap;
    va_start(ap, n);
    i32 x = va_arg(ap, i32);
    i32 y = va_arg(ap, i32);
    double z = va_arg(ap, double);
    double w = va_arg(ap, double);
    struct S3 s = va_arg(ap, struct S3);
    struct FD f = va_arg(ap, struct FD);
    struct D5 d = va_arg(ap, struct D5);
    va_end(ap);
    return n + x + y + z + w + s.a + s.b + s.c + f.a + f.b + d.a + d.e;
}

struct S3 nia_s3(struct S3 v, i32 k);
struct S6 nia_s6(struct S6 v, i32 k);
struct F3 nia_f3(struct F3 v, i32 k);
struct D2 nia_d2(struct D2 v, i32 k);
struct D5 nia_d5(struct D5 v, i32 k);
struct FD nia_fd(struct FD v, i32 k);
struct IFF nia_iff(struct IFF v, i32 k);
struct NA nia_na(struct NA v, i32 k);
struct BQ nia_bq(struct BQ v, i32 k);
struct W3 nia_w3(struct W3 v, i32 k);
i32 nia_ext(i8 a, u8 b, i16 c, u16 d);
i8 nia_ret_i8(void);
i64 nia_many(i64 a, i64 b, i64 c, i64 d, i64 e, struct P2 p, i64 f);
struct W3 nia_sret_after(i64 a, i64 b, i64 c, i64 d, i64 e, struct P2 p);

/* Returns 0, or the number of the first check that failed. */
i32 c_calls_nia(void) {
    static const u8 bytes[4] = { 0 };
    i32 id = 0;
#define CHECK(condition) do { id++; if (!(condition)) return id; } while (0)
    struct S3 s3 = nia_s3((struct S3){ 1, 2, 3 }, 10);
    CHECK(s3.a == 11 && s3.b == 12 && s3.c == 13);
    struct S6 s6 = nia_s6((struct S6){ 100, 200, 300 }, 5);
    CHECK(s6.a == 105 && s6.b == 205 && s6.c == 305);
    struct F3 f3 = nia_f3((struct F3){ 1.5f, 2.5f, 3.5f }, 1);
    CHECK(f3.a == 2.5f && f3.b == 3.5f && f3.c == 4.5f);
    struct D2 d2 = nia_d2((struct D2){ 0.25, 0.5 }, 2);
    CHECK(d2.a == 2.25 && d2.b == 2.5);
    struct D5 d5 = nia_d5((struct D5){ 1, 2, 3, 4, 5 }, 10);
    CHECK(d5.a == 11 && d5.b == 12 && d5.c == 13 && d5.d == 14 && d5.e == 15);
    struct FD fd = nia_fd((struct FD){ 0.5f, 0.75 }, 3);
    CHECK(fd.a == 3.5f && fd.b == 3.75);
    struct IFF iff = nia_iff((struct IFF){ 7, 0.5f, 1.5f }, 1);
    CHECK(iff.c == 8 && iff.a == 1.5f && iff.b == 2.5f);
    struct NA na = nia_na((struct NA){ { 1, 2 }, 3 }, 4);
    CHECK(na.p.x == 5 && na.p.y == 6 && na.z == 7);
    struct BQ bq = nia_bq((struct BQ){ 9, 1000000000000LL }, 1);
    CHECK(bq.a == 10 && bq.b == 1000000000001LL);
    struct W3 w3 = nia_w3((struct W3){ 1, 2, 3 }, 100);
    CHECK(w3.a == 101 && w3.b == 102 && w3.c == 103);
    CHECK(nia_ext(-1, 255, -2, 65535) == 65787);
    CHECK(nia_ret_i8() == -7);
    struct P2 p = { &bytes[0], &bytes[3] };
    CHECK(nia_many(1, 2, 3, 4, 5, p, 6) == 24);
    struct W3 after = nia_sret_after(1, 2, 3, 4, 5, p);
    CHECK(after.a == 3 && after.b == 7 && after.c == 8);
#undef CHECK
    return 0;
}
