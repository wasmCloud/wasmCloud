// wasi shim: arg.cpp falls to the BSD branch, which wants <sys/syslimits.h>.
#ifndef _WASI_SHIM_SYS_SYSLIMITS_H
#define _WASI_SHIM_SYS_SYSLIMITS_H

#include <limits.h>

#ifndef PATH_MAX
#define PATH_MAX 4096
#endif

#endif
