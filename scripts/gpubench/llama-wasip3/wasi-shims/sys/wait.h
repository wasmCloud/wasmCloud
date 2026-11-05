// wasi shim: sheredom/subprocess.h needs waitpid. WASI has no child processes,
// so this pairs with the spawn.h shim, which never starts one.
#ifndef _WASI_SHIM_SYS_WAIT_H
#define _WASI_SHIM_SYS_WAIT_H

#include <sys/types.h>
#include <errno.h>

#define WNOHANG 1
#define WUNTRACED 2

#define WIFEXITED(s)   (1)
#define WEXITSTATUS(s) ((s) & 0xff)
#define WIFSIGNALED(s) (0)
#define WTERMSIG(s)    (0)
#define WIFSTOPPED(s)  (0)
#define WSTOPSIG(s)    (0)

#ifdef __cplusplus
extern "C" {
#endif

static inline pid_t waitpid(pid_t, int *, int) { errno = ECHILD; return -1; }

#ifdef __cplusplus
}
#endif

#endif
