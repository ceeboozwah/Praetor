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
 *   mml_dir_*  — directory enumeration (opaque handle, i64)
 *   mml_proc_* — process spawn/wait (N65B)
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

/* ---- directories (dir domain — N65A) ----
 * `mml_dir_open` takes (ptr,len) — paths are not assumed
 * NUL-terminated — and returns an opaque handle >0 or -errno.
 * `mml_dir_next` yields one entry per call: 1 = entry, 0 = end,
 * negative = -errno. "." and ".." are skipped; names that are not
 * valid UTF-8 are invisible (every `str` is valid UTF-8 — a name
 * that cannot be one is not a name). `kind`: 1 file, 2 dir, 0 other
 * — d_type where the filesystem reports it, stat fallback where it
 * doesn't. The returned name pointer is valid only until the next
 * `mml_dir_next`/`mml_dir_close` call — callers copy immediately.
 * `mml_mkdir` creates one directory, returns 0 or -errno (EEXIST is
 * an error like any other — callers decide if it's benign).
 * `mml_remove` deletes a file or an EMPTY directory (POSIX remove()
 * semantics — it dispatches on what the path is).
 * Negative results are `-errno` on POSIX, `-GetLastError()` on Win32
 * — same sign convention, host-native code space. */
long long mml_dir_open(const char *path, long long len);
int       mml_dir_next(long long dh, const char **name,
                       long long *nlen, int *kind);
int       mml_dir_close(long long dh);
int       mml_mkdir(const char *path, long long len, int mode);
int       mml_remove(const char *path, long long len);
/* `mml_rename` moves `from` to `to`, replacing an existing `to`
 * (POSIX rename(2), Win32 MoveFileExW REPLACE_EXISTING|COPY_ALLOWED)
 * — the dep-cache tmp→final swap. 0 or -errno/-GetLastError. */
int       mml_rename(const char *from, long long flen,
                     const char *to, long long tlen);
/* Canonicalize a path — absolute, `..`/`.`/symlinks resolved
 * (POSIX realpath, Win32 GetFinalPathNameByHandle). Returns the
 * resolved length (bytes copied to buf, not NUL-terminated) or
 * -errno when the path doesn't resolve or the buffer is short. */
long long mml_realpath(const char *path, long long len,
                       char *buf, long long cap);
/* What a path points at: 1 = regular file, 2 = directory,
 * 0 = other, negative = -errno (ENOENT when it doesn't exist). */
int       mml_path_kind(const char *path, long long len);
/* Host strerror text for an errno — the loader needs Rust's
 * `{strerror} (os error {n})` io::Error spelling. Returns the
 * message length copied to buf, or a negative on bad errno. */
long long mml_strerror(int e, char *buf, long long cap);

/* ---- process spawn (proc domain — N65B) ----
 * Synchronous spawn-and-wait: `args` is a NUL-separated byte block
 * ("a\0b\0c\0" — empty means argv = {exe}), `cwd` is a (ptr,len)
 * optional working directory (len 0 = inherit). Returns the exit
 * code (0..255), 128+signal on signal death, or -errno /
 * -GetLastError() on spawn failure (the child could not exec / no
 * such exe). Stdio is inherited — capture is deliberately out of
 * v1 scope. */
long long mml_proc_run(const char *exe, long long elen,
                       const char *args, long long alen,
                       const char *cwd, long long clen);

/* `mml_proc_run_cap` — spawn-and-wait with ONE stdio stream
 * redirected to `cap` (created/truncated): `which` 1 = stdout,
 * 2 = stderr, 3 = both, 0 = inherit both. s0's `Command::output()` shape:
 * the toolchain driver's diagnostics are captured and only shown
 * when the step fails — warnings never reach the user's tty. */
long long mml_proc_run_cap(const char *exe, long long elen,
                           const char *args, long long alen,
                           const char *cwd, long long clen,
                           const char *cap, long long caplen,
                           long long which);

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
/* `mml_env_set` — setenv(name, val, 1) / SetEnvironmentVariableW.
 * The toolchain dispatcher needs OUR_DISPATCHED on a re-exec; the
 * env view stays read-only to ordinary programs. 0 or -errno. */
int       mml_env_set(const char *name, long long nlen,
                      const char *val, long long vlen);
/* `mml_self_exe` — the running executable's path (readlink
 * /proc/self/exe, _NSGetExecutablePath, GetModuleFileNameW). The
 * toolchain dispatcher scans its directory for `our-x.y.z`
 * siblings. Returns the length or -errno/-ENAMETOOLONG. */
long long mml_self_exe(char *buf, long long cap);

/* ---- argv (stashed by the platform entry point) ---- */
void        mml_stash_argv(long long argc, const void *arr);
const void *mml_argv(void);

/* ---- platform ---- */
/* The LLVM target triple this runtime object was built for — the
 * self-hosted driver reads it to pick runtime sources, link flags,
 * and the exe suffix without env vars or /proc probing. *out points
 * at static storage; the return is the byte length. */
long long   mml_target_triple(const char **out);
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
mml_chan  *mml_chan_new_bounded(size_t esz, size_t bound);
/* CHAN-003 — status-returning bounded send outcomes. */
#define MML_CHAN_SENT   0   /* enqueued; ownership transferred  */
#define MML_CHAN_CLOSED 1   /* closed before admission; caller
                               retains the value */
int        mml_chan_send_bounded(mml_chan *c, const void *src);
void       mml_chan_send(mml_chan *c, const void *src);
void       mml_chan_recv(mml_chan *c, void *dst);
/* CHAN-004 — status-returning recv: MML_CHAN_SENT = message
 * delivered into dst; MML_CHAN_CLOSED = channel closed and
 * drained, dst left untouched. Works on both channel kinds and
 * both storage layouts. */
int        mml_chan_recv_bounded(mml_chan *c, void *dst);
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
