// scenic-launcher: the login item that keeps one of the map's programs running.
//
//   scenic-launcher <name>
//
// Runs the command listed in ~/Library/Application Support/scenic/run/<name> (one argument per
// line; blank lines and lines starting with # are skipped; a leading "~/" is the home folder) as a
// child process, and starts it again whenever it exits: after 2 s, doubling up to 60 s while it
// keeps exiting within a minute of starting. The file is read afresh on every start, so changing
// what runs never touches this program. To set environment variables, start the list with
// /usr/bin/env and NAME=value lines.
//
// Built once and never rebuilt: macOS asks once whether this binary may use network volumes (the
// NAS) and remembers the answer for this exact binary. The programs it runs inherit that
// permission because they are its children (posix_spawn), never replacements (exec).
// SIGTERM, SIGINT and SIGHUP are passed on to the child; SIGTERM and SIGINT then end the launcher.
#include <errno.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

extern char **environ;
static volatile pid_t child = 0;
static volatile sig_atomic_t stopping = 0;

static void on_signal(int sig) {
    if (sig != SIGHUP) stopping = 1;
    if (child > 0) kill(child, sig);
}

// Fills argv from the file (in buf); returns the count, or -1 if it can't be read.
static int read_args(const char *path, char *buf, size_t cap, char **argv, int max) {
    FILE *f = fopen(path, "r");
    if (!f) return -1;
    size_t n = fread(buf, 1, cap - 1, f);
    fclose(f);
    buf[n] = 0;
    const char *home = getenv("HOME");
    int argc = 0;
    char *save = NULL;
    for (char *line = strtok_r(buf, "\n", &save); line && argc < max - 1; line = strtok_r(NULL, "\n", &save)) {
        size_t len = strlen(line);
        while (len && line[len - 1] == '\r') line[--len] = 0;
        if (!len || line[0] == '#') continue;
        if (line[0] == '~' && line[1] == '/' && home) {
            size_t hl = strlen(home);
            char *s = malloc(hl + len);
            if (!s) return -1;
            memcpy(s, home, hl);
            memcpy(s + hl, line + 1, len);  // the rest, with its terminating NUL
            line = s;
        }
        argv[argc++] = line;
    }
    argv[argc] = NULL;
    return argc;
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: scenic-launcher <name>\n");
        return 2;
    }
    const char *home = getenv("HOME");
    char path[2048];
    snprintf(path, sizeof path, "%s/Library/Application Support/scenic/run/%s", home ? home : "", argv[1]);
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = on_signal;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGTERM, &sa, NULL);
    sigaction(SIGINT, &sa, NULL);
    sigaction(SIGHUP, &sa, NULL);
    static char buf[65536];
    int delay = 2;
    while (!stopping) {
        char *cargv[512];
        int n = read_args(path, buf, sizeof buf, cargv, 512);
        time_t t0 = time(NULL);
        if (n > 0) {
            pid_t pid;
            int rc = posix_spawnp(&pid, cargv[0], NULL, NULL, cargv, environ);
            if (rc == 0) {
                child = pid;
                if (stopping) kill(pid, SIGTERM);
                int st;
                while (waitpid(pid, &st, 0) < 0 && errno == EINTR) {
                }
                child = 0;
            } else {
                fprintf(stderr, "scenic-launcher: can't start %s: %s\n", cargv[0], strerror(rc));
            }
        } else {
            fprintf(stderr, "scenic-launcher: nothing to run in %s\n", path);
        }
        if (stopping) break;
        delay = time(NULL) - t0 >= 60 ? 2 : (delay * 2 > 60 ? 60 : delay * 2);
        for (int i = 0; i < delay && !stopping; i++) sleep(1);
    }
    return 0;
}
