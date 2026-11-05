// wasi shim: sheredom/subprocess.h needs posix_spawn declarations. WASI cannot
// start processes, so every spawn fails and nothing here is ever reached.
#ifndef _WASI_SHIM_SPAWN_H
#define _WASI_SHIM_SPAWN_H

#include <sys/types.h>
#include <errno.h>
#include <signal.h>

typedef struct { int __unused; } posix_spawnattr_t;
typedef struct { int __unused; } posix_spawn_file_actions_t;

#ifdef __cplusplus
extern "C" {
#endif

static inline int posix_spawn(pid_t *, const char *,
                              const posix_spawn_file_actions_t *,
                              const posix_spawnattr_t *,
                              char * const *, char * const *) { return ENOTSUP; }

static inline int posix_spawnp(pid_t *, const char *,
                               const posix_spawn_file_actions_t *,
                               const posix_spawnattr_t *,
                               char * const *, char * const *) { return ENOTSUP; }

static inline int posix_spawn_file_actions_init(posix_spawn_file_actions_t *) { return 0; }
static inline int posix_spawn_file_actions_destroy(posix_spawn_file_actions_t *) { return 0; }
static inline int posix_spawn_file_actions_addclose(posix_spawn_file_actions_t *, int) { return 0; }
static inline int posix_spawn_file_actions_adddup2(posix_spawn_file_actions_t *, int, int) { return 0; }
static inline int posix_spawn_file_actions_addchdir(posix_spawn_file_actions_t *, const char *) { return 0; }
static inline int posix_spawn_file_actions_addchdir_np(posix_spawn_file_actions_t *, const char *) { return 0; }

// wasi-libc's signal emulation is compile-only and declares no kill().
static inline int kill(pid_t, int) { errno = ENOTSUP; return -1; }

#ifdef __cplusplus
}
#endif

#endif
