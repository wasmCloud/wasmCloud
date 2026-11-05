// wasi shim: console.cpp needs termios declarations to compile. WASI has no tty
// control, so the calls always fail. llama-bench never pulls this object in.
#ifndef _WASI_SHIM_TERMIOS_H
#define _WASI_SHIM_TERMIOS_H

#include <errno.h>

#define NCCS 32

typedef unsigned int tcflag_t;
typedef unsigned char cc_t;

struct termios {
    tcflag_t c_iflag;
    tcflag_t c_oflag;
    tcflag_t c_cflag;
    tcflag_t c_lflag;
    cc_t     c_cc[NCCS];
};

#define ICANON  0x0002
#define ECHO    0x0008
#define VMIN    6
#define VTIME   5
#define TCSANOW 0

// wasi-libc ships <sys/ioctl.h> but no tty ioctls. console.cpp includes it
// before this header, so the window-size bits land here.
#ifndef TIOCGWINSZ
#define TIOCGWINSZ 0x5413
struct winsize {
    unsigned short ws_row;
    unsigned short ws_col;
    unsigned short ws_xpixel;
    unsigned short ws_ypixel;
};
#endif

#ifdef __cplusplus
extern "C" {
#endif

static inline int tcgetattr(int, struct termios *) { errno = ENOTSUP; return -1; }
static inline int tcsetattr(int, int, const struct termios *) { errno = ENOTSUP; return -1; }

#ifdef __cplusplus
}
#endif

#endif
