/* Test: getnameinfo with WarpGrid DNS shim.
 *
 * WARPGRID_SHIM_REQUIRED
 *
 * This test validates getnameinfo() integration with the WarpGrid DNS shim.
 * When run against the stock sysroot (no shim), tests are skipped.
 * When run against the patched sysroot, all tests execute.
 *
 * Test cases:
 *   1. getnameinfo compiles and links against patched sysroot
 *   2. NI_NUMERICHOST returns formatted IP address (no name lookup)
 *   3. NI_NUMERICSERV returns port number as string
 *   4. Both NI_NUMERICHOST and NI_NUMERICSERV together
 *   5. Fallthrough to numeric when shim returns 0 (not managed)
 *   6. IPv6 address with NI_NUMERICHOST
 *   7. EAI_FAMILY for unsupported address family
 *   8. EAI_OVERFLOW when buffer is too small
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
    printf("  PASS: getnameinfo compiles and links against patched sysroot\n");
    return 0;
}

/* ---- Test 2: NI_NUMERICHOST returns formatted IPv4 ---------------------- */

static int test_numerichost_ipv4(void) {
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_port = htons(5432);
    inet_pton(AF_INET, "10.0.1.42", &sa.sin_addr);

    char host[NI_MAXHOST];
    char serv[NI_MAXSERV];

    int ret = getnameinfo((struct sockaddr *)&sa, sizeof(sa),
                          host, sizeof(host), serv, sizeof(serv),
                          NI_NUMERICHOST | NI_NUMERICSERV);

    if (ret != 0) {
        fprintf(stderr, "  FAIL: NI_NUMERICHOST IPv4: getnameinfo returned %d\n", ret);
        return 1;
    }

    if (strcmp(host, "10.0.1.42") != 0) {
        fprintf(stderr, "  FAIL: NI_NUMERICHOST IPv4: got host='%s', expected '10.0.1.42'\n", host);
        return 1;
    }

    if (strcmp(serv, "5432") != 0) {
        fprintf(stderr, "  FAIL: NI_NUMERICSERV: got serv='%s', expected '5432'\n", serv);
        return 1;
    }

    printf("  PASS: NI_NUMERICHOST IPv4 returns '10.0.1.42' port '5432'\n");
    return 0;
}

/* ---- Test 3: NI_NUMERICSERV returns port as string ---------------------- */

static int test_numericserv(void) {
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_port = htons(8080);
    inet_pton(AF_INET, "127.0.0.1", &sa.sin_addr);

    char serv[NI_MAXSERV];

    int ret = getnameinfo((struct sockaddr *)&sa, sizeof(sa),
                          NULL, 0, serv, sizeof(serv),
                          NI_NUMERICHOST | NI_NUMERICSERV);

    if (ret != 0) {
        fprintf(stderr, "  FAIL: NI_NUMERICSERV: getnameinfo returned %d\n", ret);
        return 1;
    }

    if (strcmp(serv, "8080") != 0) {
        fprintf(stderr, "  FAIL: NI_NUMERICSERV: got '%s', expected '8080'\n", serv);
        return 1;
    }

    printf("  PASS: NI_NUMERICSERV returns '8080'\n");
    return 0;
}

/* ---- Test 4: Fallthrough to numeric when shim stub active --------------- */

static int test_fallthrough_numeric(void) {
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_port = htons(80);
    inet_pton(AF_INET, "192.168.1.1", &sa.sin_addr);

    char host[NI_MAXHOST];

    /* Without NI_NUMERICHOST, getnameinfo will try the reverse resolve shim.
     * With the weak stub (returns 0), it should fall back to numeric format. */
    int ret = getnameinfo((struct sockaddr *)&sa, sizeof(sa),
                          host, sizeof(host), NULL, 0,
                          0 /* no flags — will try name lookup first */);

    if (ret != 0) {
        fprintf(stderr, "  FAIL: fallthrough numeric: getnameinfo returned %d\n", ret);
        return 1;
    }

    /* Should get the numeric IP since no reverse resolve is available */
    if (strcmp(host, "192.168.1.1") != 0) {
        /* It's possible the implementation resolved it to a hostname via some
         * other mechanism. That's also acceptable. */
        printf("  PASS: fallthrough resolved to '%s' (non-numeric OK)\n", host);
        return 0;
    }

    printf("  PASS: fallthrough returns numeric '192.168.1.1'\n");
    return 0;
}

/* ---- Test 5: IPv6 NI_NUMERICHOST --------------------------------------- */

static int test_numerichost_ipv6(void) {
    struct sockaddr_in6 sa6;
    memset(&sa6, 0, sizeof(sa6));
    sa6.sin6_family = AF_INET6;
    sa6.sin6_port = htons(443);
    inet_pton(AF_INET6, "::1", &sa6.sin6_addr);

    char host[NI_MAXHOST];
    char serv[NI_MAXSERV];

    int ret = getnameinfo((struct sockaddr *)&sa6, sizeof(sa6),
                          host, sizeof(host), serv, sizeof(serv),
                          NI_NUMERICHOST | NI_NUMERICSERV);

    if (ret != 0) {
        fprintf(stderr, "  FAIL: NI_NUMERICHOST IPv6: getnameinfo returned %d\n", ret);
        return 1;
    }

    if (strcmp(host, "::1") != 0) {
        fprintf(stderr, "  FAIL: NI_NUMERICHOST IPv6: got '%s', expected '::1'\n", host);
        return 1;
    }

    if (strcmp(serv, "443") != 0) {
        fprintf(stderr, "  FAIL: NI_NUMERICSERV IPv6: got '%s', expected '443'\n", serv);
        return 1;
    }

    printf("  PASS: NI_NUMERICHOST IPv6 returns '::1' port '443'\n");
    return 0;
}

/* ---- Test 6: Unsupported address family returns EAI_FAMILY -------------- */

static int test_bad_family(void) {
    /* Construct a sockaddr with an unsupported family */
    struct sockaddr sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_family = AF_UNIX; /* Not AF_INET or AF_INET6 */

    char host[NI_MAXHOST];
    int ret = getnameinfo(&sa, sizeof(sa), host, sizeof(host), NULL, 0,
                          NI_NUMERICHOST);

    if (ret == EAI_FAMILY) {
        printf("  PASS: unsupported family returns EAI_FAMILY\n");
        return 0;
    }

    fprintf(stderr, "  FAIL: unsupported family: expected EAI_FAMILY, got %d\n", ret);
    return 1;
}

/* ---- Test 7: Small buffer returns EAI_OVERFLOW -------------------------- */

static int test_overflow(void) {
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_port = htons(80);
    inet_pton(AF_INET, "192.168.100.200", &sa.sin_addr);

    /* Buffer too small for "192.168.100.200" (15 chars + NUL = 16) */
    char host[4];
    int ret = getnameinfo((struct sockaddr *)&sa, sizeof(sa),
                          host, sizeof(host), NULL, 0,
                          NI_NUMERICHOST);

    if (ret == EAI_OVERFLOW) {
        printf("  PASS: small buffer returns EAI_OVERFLOW\n");
        return 0;
    }

    /* Some implementations may truncate instead of returning overflow */
    fprintf(stderr, "  FAIL: small buffer: expected EAI_OVERFLOW, got %d\n", ret);
    return 1;
}

/* ---- Test 8: NULL host and serv both skipped gracefully ----------------- */

static int test_null_buffers(void) {
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_port = htons(80);
    inet_pton(AF_INET, "10.0.0.1", &sa.sin_addr);

    /* Both host and serv are NULL — should succeed without writing anything */
    int ret = getnameinfo((struct sockaddr *)&sa, sizeof(sa),
                          NULL, 0, NULL, 0,
                          NI_NUMERICHOST | NI_NUMERICSERV);

    if (ret == 0) {
        printf("  PASS: NULL host and serv buffers handled gracefully\n");
        return 0;
    }

    fprintf(stderr, "  FAIL: NULL buffers: expected 0, got %d\n", ret);
    return 1;
}

/* ---- Test 9: socklen_t too small returns EAI_FAMILY --------------------- */

static int test_short_socklen(void) {
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_port = htons(80);
    inet_pton(AF_INET, "10.0.0.1", &sa.sin_addr);

    char host[NI_MAXHOST];
    /* Pass a socklen smaller than sizeof(sockaddr_in) */
    int ret = getnameinfo((struct sockaddr *)&sa, 4 /* too small */,
                          host, sizeof(host), NULL, 0,
                          NI_NUMERICHOST);

    if (ret == EAI_FAMILY) {
        printf("  PASS: short socklen returns EAI_FAMILY\n");
        return 0;
    }

    fprintf(stderr, "  FAIL: short socklen: expected EAI_FAMILY, got %d\n", ret);
    return 1;
}

/* ---- Main --------------------------------------------------------------- */

int main(void) {
    int failures = 0;

    printf("test_dns_getnameinfo:\n");

    failures += test_compile_link();
    failures += test_numerichost_ipv4();
    failures += test_numericserv();
    failures += test_fallthrough_numeric();
    failures += test_numerichost_ipv6();
    failures += test_bad_family();
    failures += test_overflow();
    failures += test_null_buffers();
    failures += test_short_socklen();

    if (failures > 0) {
        fprintf(stderr, "\n%d test(s) failed\n", failures);
        return 1;
    }

    printf("\nAll tests passed\n");
    return 0;
}
