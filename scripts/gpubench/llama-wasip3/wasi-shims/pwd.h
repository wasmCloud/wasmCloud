// wasi shim: hf-cache.cpp uses getpwuid to find a home dir. WASI has no user
// database, so lookups fail and the caller falls back to $HOME.
#ifndef _WASI_SHIM_PWD_H
#define _WASI_SHIM_PWD_H

#include <sys/types.h>
#include <stddef.h>

struct passwd {
    char * pw_name;
    char * pw_dir;
    char * pw_shell;
};

#ifdef __cplusplus
extern "C" {
#endif

static inline struct passwd * getpwuid(uid_t) { return NULL; }

// wasi-libc has no getuid either; the result only feeds getpwuid above.
static inline uid_t getuid(void) { return 0; }

#ifdef __cplusplus
}
#endif

#endif
