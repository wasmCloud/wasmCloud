// wasi shim, SHADOWS the sysroot header (searched with -I, before the sysroot).
//
// wasi-libc declares getifaddrs/freeifaddrs but ships no implementation, so
// cpp-httplib fails to link. Inline stubs keep the symbols out of the object.
// Only cpp-httplib's interface-enumeration path calls these.
#ifndef _WASI_SHIM_IFADDRS_H
#define _WASI_SHIM_IFADDRS_H

#include <sys/socket.h>
#include <errno.h>
#include <stddef.h>

struct ifaddrs {
    struct ifaddrs  * ifa_next;
    char            * ifa_name;
    unsigned int      ifa_flags;
    struct sockaddr * ifa_addr;
    struct sockaddr * ifa_netmask;
    union {
        struct sockaddr * ifu_broadaddr;
        struct sockaddr * ifu_dstaddr;
    } ifa_ifu;
    void            * ifa_data;
};

#define ifa_broadaddr ifa_ifu.ifu_broadaddr
#define ifa_dstaddr   ifa_ifu.ifu_dstaddr

#ifdef __cplusplus
extern "C" {
#endif

static inline int  getifaddrs(struct ifaddrs ** p) { if (p) { *p = NULL; } errno = ENOTSUP; return -1; }
static inline void freeifaddrs(struct ifaddrs *) { }

#ifdef __cplusplus
}
#endif

#endif
