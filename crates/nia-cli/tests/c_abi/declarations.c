/* Every C value shape that crosses an extern boundary; `declarations.nia`
   declares the same functions. */
#include <stdint.h>
#include <stddef.h>
struct S3 { uint8_t a, b, c; };
struct S5 { uint8_t a[5]; };
struct S6 { uint16_t a, b, c; };
struct F1 { float a; };
struct F3 { float a, b, c; };
struct D1 { double a; };
struct D3 { double a, b, c; };
struct D4 { double a, b, c, d; };
struct D5 { double a, b, c, d, e; };
struct FD { float a; double b; };
struct DF { double a; float b; };
struct FFI { float a, b; int32_t c; };
struct IFF { int32_t c; float a, b; };
struct P2 { void *a, *b; };
struct NA { struct { float x, y; } p; float z; };
struct AR { float v[3]; };
#ifdef __SIZEOF_INT128__
struct Q { __int128 a; };
#endif
struct BQ { uint8_t a; int64_t b; };
struct US { uint16_t a; uint8_t b; };
typedef void (*cb)(int);
struct FP { cb f; int32_t n; };

int8_t r_i8(void); uint8_t r_u8(void); int16_t r_i16(void); uint16_t r_u16(void);
int32_t r_i32(void); uint32_t r_u32(void); int64_t r_i64(void); float r_f32(void); double r_f64(void);
void *r_ptr(void); size_t r_usize(void);
void t_scalars(int8_t, uint8_t, int16_t, uint16_t, int32_t, uint32_t, int64_t, float, double, void*, size_t);
#ifdef __SIZEOF_INT128__
__int128 r_i128(void); void t_i128(__int128); void t_q(struct Q); struct Q r_q(void);
#endif
void t_s3(struct S3); struct S3 r_s3(void); void t_s5(struct S5); struct S5 r_s5(void);
void t_s6(struct S6); struct S6 r_s6(void);
void t_f1(struct F1); struct F1 r_f1(void); void t_f3(struct F3); struct F3 r_f3(void);
void t_d1(struct D1); struct D1 r_d1(void); void t_d3(struct D3); struct D3 r_d3(void);
void t_d4(struct D4); struct D4 r_d4(void); void t_d5(struct D5); struct D5 r_d5(void);
void t_fd(struct FD); struct FD r_fd(void); void t_df(struct DF); struct DF r_df(void);
void t_ffi(struct FFI); struct FFI r_ffi(void); void t_iff(struct IFF); struct IFF r_iff(void);
void t_p2(struct P2); struct P2 r_p2(void); void t_na(struct NA); struct NA r_na(void);
void t_ar(struct AR); struct AR r_ar(void); void t_bq(struct BQ); struct BQ r_bq(void);
void t_us(struct US); struct US r_us(void); void t_fp(struct FP); struct FP r_fp(void);
struct P2 t_sret_ints(int64_t, int64_t, int64_t, int64_t, int64_t, struct P2);
void t_ints6(int64_t, int64_t, int64_t, int64_t, int64_t, struct P2, int64_t);
void t_ints5(int64_t, int64_t, int64_t, int64_t, int64_t, struct P2, int64_t);
void t_ints4(int64_t, int64_t, int64_t, int64_t, struct P2, int64_t);
void t_sse7(double, double, double, double, double, double, double, struct D1, struct FD, double);
void t_sse8(double, double, double, double, double, double, double, double, struct FD, struct D1);
void t_mixed_exhaust(int64_t, int64_t, int64_t, int64_t, int64_t, struct FD, int64_t);
int t_var(int, ...);

void use(void) {
  struct S3 s3={0}; struct S5 s5={0}; struct S6 s6={0}; struct F1 f1={0}; struct F3 f3={0};
  struct D1 d1={0}; struct D3 d3={0}; struct D4 d4={0}; struct D5 d5={0}; struct FD fd={0}; struct DF df={0};
  struct FFI ffi={0}; struct IFF iff={0}; struct P2 p2={0}; struct NA na={0}; struct AR ar={0}; 
#ifdef __SIZEOF_INT128__
struct Q q={0};
#endif

  struct BQ bq={0}; struct US us={0}; struct FP fp={0};
  r_i8(); r_u8(); r_i16(); r_u16(); r_i32(); r_u32(); r_i64(); r_f32(); r_f64(); r_ptr(); r_usize();
  t_scalars(0,0,0,0,0,0,0,0,0,0,0);
#ifdef __SIZEOF_INT128__
 r_i128(); t_i128(0); t_q(q); r_q();
#endif

  t_s3(s3); r_s3(); t_s5(s5); r_s5(); t_s6(s6); r_s6(); t_f1(f1); r_f1(); t_f3(f3); r_f3();
  t_d1(d1); r_d1(); t_d3(d3); r_d3(); t_d4(d4); r_d4(); t_d5(d5); r_d5(); t_fd(fd); r_fd(); t_df(df); r_df();
  t_ffi(ffi); r_ffi(); t_iff(iff); r_iff(); t_p2(p2); r_p2(); t_na(na); r_na(); t_ar(ar); r_ar();
  t_bq(bq); r_bq(); t_us(us); r_us(); t_fp(fp); r_fp();
  t_sret_ints(0,0,0,0,0,p2); t_ints6(0,0,0,0,0,p2,0); t_ints5(0,0,0,0,0,p2,0); t_ints4(0,0,0,0,p2,0);
  t_sse7(0,0,0,0,0,0,0,d1,fd,0); t_sse8(0,0,0,0,0,0,0,0,fd,d1); t_mixed_exhaust(0,0,0,0,0,fd,0);
  t_var(1, (int8_t)2, (uint16_t)3, 3.0f, 4.0, s3, fd, p2, d5);
}
