/* mmlrt.h — the stable runtime contract shared by mmlrt_posix.c and
 * mmlrt_win32.c (N58C-2). The compiler lowers into these names and the
 * stdlib declares them as externs; each platform file implements the
 * whole surface. OS-native structs, constants, and calling quirks
 * terminate here — nothing above this line ever sees HANDLE, SOCKET,
 * pthread_t, O_* bit values, or UTF-16.
 *
 * Namespace guardrail (N58C):
 *   mml_fd_*   — the file-descriptor domain (CRT/POSIX fds, i32)
 *   mml_sock_* — the socket domain (SOCKET on Windows, i64 handles)
 * These are different resources even where they fit in the same int;
 * nothing may cross-call between them.
 */
#ifndef MMLRT_H
#define MMLRT_H

#include <stddef.h>

/* The language's `str`/`slice` element view: {ptr, len}. */
typedef struct { const char *ptr; long long len; } mml_str_t;

/* Portable open-flag bits — the language-level vocabulary that
 * mml_fd_open_flags translates to the host's O_* constants. */
#define MML_OF_READ   1
#define MML_OF_WRITE  2
#define MML_OF_CREATE 4
#define MML_OF_TRUNC  8
#define MML_OF_APPEND 16

/* ---- files (fd domain) ---- */
int       mml_fd_open_mode(const char *path, int flags, int mode);
int       mml_fd_open_flags(const char *path, int oflags, int mode);
long long mml_fd_read(int fd, void *buf, long long n);
long long mml_fd_write(int fd, const void *buf, long long n);
int       mml_fd_close(int fd);
int       mml_errno(void);

/* ---- sockets (sock domain — i64 handles; SOCKET on Windows) ---- */
long long mml_sock_socket(int domain, int ty, int proto);
int       mml_sock_bind(long long s, const void *addr, unsigned len);
int       mml_sock_connect(long long s, const void *addr, unsigned len);
int       mml_sock_listen(long long s, int backlog);
long long mml_sock_accept(long long s);
int       mml_sock_reuse_addr(long long s);
long long mml_sock_recv(long long s, void *buf, long long n);
long long mml_sock_send(long long s, const void *buf, long long n);
int       mml_sock_close(long long s);
int       mml_sock_errno(void);
void      mml_sockaddr_v4(unsigned char *sa, int o0, int o1, int o2,
                          int o3, int port);

/* ---- process ---- */
int       mml_getpid(void);
int       mml_yield(void);
int       mml_sleep_us(int usec);
void      mml_exit(int code);
int       mml_env_get(const char *name, long long nlen,
                      const char **out_ptr, long long *out_len);
long long mml_getcwd(char *buf, long long cap);

/* ---- argv (stashed by the platform entry point) ---- */
void        mml_stash_argv(long long argc, const void *arr);
const void *mml_argv(void);
long long   mml_argc(void);
int         mml_arg(long long i, const char **out_ptr, long long *out_len);

/* ---- concurrency (opaque object pointers) ---- */
typedef void *(*mml_task_fn)(void *env);
typedef struct mml_task  mml_task;
typedef struct mml_mutex mml_mutex;
typedef struct mml_chan  mml_chan;

mml_task  *mml_task_spawn(mml_task_fn fn, void *env);
void       mml_task_join(mml_task *t);
mml_mutex *mml_mutex_new(size_t payload_sz);
mml_mutex *mml_mutex_wrap(void *p);
void      *mml_mutex_data(mml_mutex *m);
void      *mml_mutex_lock(mml_mutex *m);
void       mml_mutex_unlock(mml_mutex *m);
mml_chan  *mml_chan_new(size_t esz);
void       mml_chan_send(mml_chan *c, const void *src);
void       mml_chan_recv(mml_chan *c, void *dst);
void       mml_chan_close(mml_chan *c);

/* ---- checks, strings, allocation (platform-neutral: mmlrt_common.c) ---- */
void mml_bounds_check(long long idx, long long len);
void mml_bounds_fail(long long idx, long long len);   /* noreturn */
void mml_shift_check(long long amt, long long width);
void mml_range_check(long long lo, long long hi, long long limit);
int  mml_str_eq(const void *a, long long alen, const void *b, long long blen);
void mml_str_validate(const void *p, long long len);  /* traps */
int  mml_str_check(const void *p, long long len);
void *mml_alloc(long long size);
void *mml_alloc_slice(long long size, long long count);
void  mml_free(void *p);

/* The generated program's entry. */
extern long long mml_main_args(const mml_str_t *args, long long len);

#endif /* MMLRT_H */
