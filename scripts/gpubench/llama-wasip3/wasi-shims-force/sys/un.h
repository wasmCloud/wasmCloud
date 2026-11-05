// wasi shim, SHADOWS the sysroot header (searched with -I, before the sysroot).
//
// wasi-libc defines sockaddr_un without sun_path ("WASI has no UNIX-domain
// sockets"), which cpp-httplib's unconditional AF_UNIX code cannot compile
// against. Give it the full struct so the TU builds. Only the server/AF_UNIX
// paths touch these fields, and llama-bench never reaches them.
#ifndef _WASI_SHIM_SYS_UN_H
#define _WASI_SHIM_SYS_UN_H

#include <bits/alltypes.h>

struct sockaddr_un {
    sa_family_t sun_family;
    char        sun_path[108];
};

#define SUN_LEN(s) (2 + strlen((s)->sun_path))

#endif
