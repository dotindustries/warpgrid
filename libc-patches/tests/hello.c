/* Minimal C test program to validate wasi-libc sysroot.
 *
 * This program is compiled against both the stock and patched sysroots
 * to verify that the build pipeline produces a valid, linkable sysroot.
 *
 * Expected output when run in Wasmtime:
 *   wasi-libc sysroot OK
 *   exit 0
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(void) {
    /* Basic stdio - verifies libc I/O is linked */
    printf("wasi-libc sysroot OK\n");

    /* Basic string ops - verifies string functions are linked */
    const char *msg = "warpgrid";
    if (strlen(msg) != 8) {
        fprintf(stderr, "strlen failed\n");
        return 1;
    }

    /* Basic memory ops - verifies malloc/free are linked */
    char *buf = malloc(64);
    if (buf == NULL) {
        fprintf(stderr, "malloc failed\n");
        return 1;
    }
    memset(buf, 0, 64);
    snprintf(buf, 64, "hello from %s", msg);
    if (strcmp(buf, "hello from warpgrid") != 0) {
        fprintf(stderr, "snprintf/strcmp failed\n");
        free(buf);
        return 1;
    }
    free(buf);

    return 0;
}
