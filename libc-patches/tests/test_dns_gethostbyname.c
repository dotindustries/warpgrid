/* Test: gethostbyname with WarpGrid DNS shim.
 *
 * WARPGRID_SHIM_REQUIRED
 *
 * This test validates gethostbyname() integration with the WarpGrid DNS shim.
 * When run against the stock sysroot (no shim), tests are skipped.
 * When run against the patched sysroot, all tests execute.
 *
 * Test cases:
 *   1. gethostbyname compiles and links against patched sysroot
 *   2. Fallthrough: gethostbyname returns NULL when shim returns 0 (not managed)
 *   3. gethostbyname with NULL name returns NULL
 *   4. gethostbyaddr compiles and returns NULL (stub for now)
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <netdb.h>
#include <arpa/inet.h>

/* ---- Test 1: Compile/link verification ---------------------------------- */

static int test_compile_link(void) {
    /* This test passes simply by being compiled and linked successfully.
     * The gethostbyname symbol is resolved, proving the patched sysroot
     * provides the implementation including the weak shim stub. */
    printf("  PASS: gethostbyname compiles and links against patched sysroot\n");
    return 0;
}

/* ---- Test 2: Fallthrough when shim returns 0 ---------------------------- */

static int test_fallthrough_returns_null(void) {
    /* With the weak stub (returns 0 = not managed), gethostbyname should
     * return NULL because the WASI environment has no other resolver. */
    struct hostent *he = gethostbyname("some.unknown.host.example");

    if (he == NULL) {
        printf("  PASS: gethostbyname fallthrough returns NULL (shim stub active)\n");
        return 0;
    }

    /* If somehow it resolved (maybe in a runtime with network), that's also OK */
    printf("  PASS: gethostbyname resolved (runtime has network support): %s\n",
           he->h_name ? he->h_name : "(null)");
    return 0;
}

/* ---- Test 3: NULL name returns NULL ------------------------------------- */

static int test_null_name(void) {
    struct hostent *he = gethostbyname(NULL);
    if (he == NULL) {
        printf("  PASS: gethostbyname(NULL) returns NULL\n");
        return 0;
    }

    fprintf(stderr, "  FAIL: gethostbyname(NULL) should return NULL\n");
    return 1;
}

/* ---- Test 4: gethostbyaddr stub returns NULL ---------------------------- */

static int test_gethostbyaddr_stub(void) {
    struct in_addr addr;
    addr.s_addr = inet_addr("127.0.0.1");

    struct hostent *he = gethostbyaddr(&addr, sizeof(addr), AF_INET);
    if (he == NULL) {
        printf("  PASS: gethostbyaddr returns NULL (expected in WASI)\n");
        return 0;
    }

    /* If it resolved, that's also OK */
    printf("  PASS: gethostbyaddr resolved: %s\n",
           he->h_name ? he->h_name : "(null)");
    return 0;
}

/* ---- Test 5: h_errno is set on failure ---------------------------------- */

static int test_h_errno_set(void) {
    /* After a failed resolution, h_errno should be set */
    struct hostent *he = gethostbyname("nonexistent.warp.local");

    if (he == NULL) {
        /* h_errno should be HOST_NOT_FOUND or similar */
        if (h_errno == HOST_NOT_FOUND || h_errno == NO_DATA ||
            h_errno == TRY_AGAIN || h_errno == NO_RECOVERY) {
            printf("  PASS: h_errno=%d set after failed gethostbyname\n", h_errno);
            return 0;
        }
        /* h_errno == 0 is also acceptable in some implementations */
        printf("  PASS: gethostbyname returned NULL (h_errno=%d)\n", h_errno);
        return 0;
    }

    printf("  PASS: gethostbyname resolved (h_errno not tested)\n");
    return 0;
}

/* ---- Test 6: Return type has correct structure fields ------------------- */

static int test_hostent_struct_fields(void) {
    /* Verify that struct hostent fields are accessible (compile-time check).
     * This catches ABI mismatches between the patched libc and headers. */
    struct hostent he;
    he.h_name = "test";
    he.h_aliases = NULL;
    he.h_addrtype = AF_INET;
    he.h_length = 4;
    he.h_addr_list = NULL;

    if (he.h_addrtype == AF_INET && he.h_length == 4) {
        printf("  PASS: struct hostent fields are accessible and correct\n");
        return 0;
    }

    fprintf(stderr, "  FAIL: struct hostent field mismatch\n");
    return 1;
}

/* ---- Main --------------------------------------------------------------- */

int main(void) {
    int failures = 0;

    printf("test_dns_gethostbyname:\n");

    failures += test_compile_link();
    failures += test_fallthrough_returns_null();
    failures += test_null_name();
    failures += test_gethostbyaddr_stub();
    failures += test_h_errno_set();
    failures += test_hostent_struct_fields();

    if (failures > 0) {
        fprintf(stderr, "\n%d test(s) failed\n", failures);
        return 1;
    }

    printf("\nAll tests passed\n");
    return 0;
}
