#ifndef GAMELINK_DESKTOP_H
#define GAMELINK_DESKTOP_H
#include <stddef.h>
#include <stdint.h>
/* ABI v1. Input is borrowed until return; at most 32768 bytes. NULL is valid
 * only with length zero. JSON response: {"ok":value} or {"error":"message"}.
 * Output is C malloc-owned (never Go memory), at most 4194304 bytes. Release
 * exactly once using the matching free function; never retain interior pointers.
 * Calls may be concurrent. init/shutdown serialize ownership and wait for active
 * calls; double init fails, shutdown is idempotent. No handles escape Go.
 * Rust polls updates (servers=1, activity=2, settings=4), not Go callbacks.
 */
typedef struct { uint8_t *ptr; size_t len; } DesktopBuffer;
DesktopBuffer gamelink_desktop_call(const uint8_t *input, size_t length);
void gamelink_desktop_free(DesktopBuffer buffer);
#endif
