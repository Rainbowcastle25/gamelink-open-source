//go:build rust_musl

// musl invokes init_array functions with no arguments. Go's Linux library
// constructor expects glibc's argc/argv convention. build-agent-shim.sh removes
// that constructor and exposes it here for explicit, once-only host startup.
#include <stdint.h>
#include <stdlib.h>
#include <pthread.h>
extern char **environ;
extern void gamelink_go_runtime_init(int argc, char **argv);
static pthread_once_t started = PTHREAD_ONCE_INIT;
static void start_runtime(void) {
    size_t count = 0;
    while (environ[count]) count++;
    // argv[0], argv terminator, env entries, env terminator, AT_NULL pair.
    // Go retains this process-lifetime vector. No Go pointers are involved.
    uintptr_t *vector = calloc(count + 5, sizeof(uintptr_t));
    if (!vector) abort();
    vector[0] = (uintptr_t)"gamelink-agent";
    for (size_t i = 0; i < count; i++) vector[i + 2] = (uintptr_t)environ[i];
    gamelink_go_runtime_init(1, (char **)vector);
}
void gamelink_agent_runtime_start(void) { pthread_once(&started, start_runtime); }
