/* uclibc_compat.c — shims for glibc symbols that the glibc-target Rust
 * toolchain references but uClibc (rockchip830 toolchain) does not export.
 *
 * uClibc's <spawn.h> defines the posix_spawnattr_* / file_actions init &
 * destroy functions as *static inline* (only posix_spawn itself is in
 * libc.so.0), so objects compiled against glibc headers (Rust std) come
 * here with undefined references.  We must NOT include <spawn.h> (its
 * static inline definitions would clash), so the uClibc struct layouts
 * are replicated here — they are prefix-identical to glibc's, only
 * sigset_t is smaller (8 bytes vs 128), which is exactly what uClibc's
 * posix_spawn() expects to read.
 */
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

/* --- getauxval: uClibc lacks it; read /proc/self/auxv instead --- */
unsigned long getauxval(unsigned long type) {
    /* 32-bit target: auxv entries are pairs of 32-bit words. */
    struct { unsigned long a_type; unsigned long a_val; } ent;
    unsigned long ret = 0;
    int fd = open("/proc/self/auxv", O_RDONLY | O_CLOEXEC);
    if (fd < 0) {
        errno = ENOENT;
        return 0;
    }
    while (read(fd, &ent, sizeof(ent)) == (ssize_t)sizeof(ent)) {
        if (ent.a_type == type) { ret = ent.a_val; break; }
        if (ent.a_type == 0 /* AT_NULL */) break;
    }
    close(fd);
    if (!ret)
        errno = ENOENT;
    return ret;
}

/* --- uClibc spawn.h layouts (kept in sync manually) --- */
struct sched_param_uclibc { int sched_priority; };

typedef struct {
    short int __flags;
    pid_t __pgrp;
    sigset_t __sd;
    sigset_t __ss;
    struct sched_param_uclibc __sp;
    int __policy;
    int __pad[16];
} uclibc_posix_spawnattr_t;

typedef struct {
    int __allocated;
    int __used;
    void *__actions;
    int __pad[16];
} uclibc_posix_spawn_file_actions_t;

/* The symbol signatures must match the glibc declarations the callers
 * were compiled with; the structs are layout-compatible prefixes. */
#define posix_spawnattr_t             uclibc_posix_spawnattr_t
#define posix_spawn_file_actions_t    uclibc_posix_spawn_file_actions_t

int posix_spawnattr_init(posix_spawnattr_t *attr) {
    memset(attr, 0, sizeof(*attr));
    return 0;
}

int posix_spawnattr_destroy(posix_spawnattr_t *attr) {
    (void)attr;
    return 0;
}

int posix_spawnattr_getflags(const posix_spawnattr_t *attr, short *flags) {
    *flags = attr->__flags;
    return 0;
}

int posix_spawnattr_setflags(posix_spawnattr_t *attr, short flags) {
    attr->__flags = flags;
    return 0;
}

int posix_spawnattr_getpgroup(const posix_spawnattr_t *attr, pid_t *pgrp) {
    *pgrp = attr->__pgrp;
    return 0;
}

int posix_spawnattr_setpgroup(posix_spawnattr_t *attr, pid_t pgrp) {
    attr->__pgrp = pgrp;
    return 0;
}

int posix_spawnattr_getsigdefault(const posix_spawnattr_t *attr, sigset_t *sd) {
    *sd = attr->__sd;
    return 0;
}

int posix_spawnattr_setsigdefault(posix_spawnattr_t *attr, const sigset_t *sd) {
    attr->__sd = *sd;
    return 0;
}

int posix_spawnattr_getsigmask(const posix_spawnattr_t *attr, sigset_t *ss) {
    *ss = attr->__ss;
    return 0;
}

int posix_spawnattr_setsigmask(posix_spawnattr_t *attr, const sigset_t *ss) {
    attr->__ss = *ss;
    return 0;
}

int posix_spawn_file_actions_init(posix_spawn_file_actions_t *fa) {
    memset(fa, 0, sizeof(*fa));
    return 0;
}

int posix_spawn_file_actions_destroy(posix_spawn_file_actions_t *fa) {
    free(fa->__actions);
    fa->__actions = NULL;
    fa->__allocated = 0;
    fa->__used = 0;
    return 0;
}
