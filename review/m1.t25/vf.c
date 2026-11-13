/* A process in uninterruptible sleep without any stuck device: the
 * parent of a vfork waits in the kernel (state D) until the child ends. */
#define _GNU_SOURCE
#include <unistd.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <time.h>
int main(int argc, char **argv) {
    int secs = argc > 1 ? atoi(argv[1]) : 120;
    pid_t p = vfork();
    if (p == 0) {
        struct timespec t = { secs, 0 };
        syscall(SYS_nanosleep, &t, 0);
        _exit(0);
    }
    return 0;
}
