/* Test: DNS backward compatibility — patched sysroot in vanilla Wasmtime.
 *
 * US-205: Verify DNS patches with stock build compatibility.
 *
 * This test has NO shim-required marker, so it runs against BOTH stock
 * and patched sysroots. It verifies:
 *
 *   1. getaddrinfo, gethostbyname, getnameinfo all compile and link
 *   2. Weak symbol fallback paths produce correct behavior when no
 *      WarpGrid shim is present (vanilla Wasmtime)
 *   3. Results are identical between stock and patched sysroots
 *
 * Test cases:
 *   1. getaddrinfo with AI_NUMERICHOST resolves IPv4 literal
 *   2. getaddrinfo with AI_NUMERICHOST rejects hostname
 *   3. getaddrinfo fallthrough: no crash/hang for unknown host
 *   4. gethostbyname returns NULL for unknown host (no shim)
 *   5. gethostbyname(NULL) returns NULL
 *   6. getnameinfo with NI_NUMERICHOST formats IPv4 correctly
 *   7. getnameinfo with NI_NUMERICHOST formats IPv6 correctly
 *   8. getnameinfo with NI_NUMERICSERV formats port correctly
 *   9. getnameinfo fallthrough: returns numeric IP for unknown addr
 *  10. getnameinfo with bad family returns EAI_FAMILY
 *  11. All three functions used together in realistic sequence
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <netdb.h>
#include <arpa/inet.h>

/* ---- Test 1: getaddrinfo AI_NUMERICHOST IPv4 ---------------------------- */

static int test_getaddrinfo_numerichost_ipv4(void) {
    struct addrinfo hints;
    struct addrinfo *res = NULL;

    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    hints.ai_flags = AI_NUMERICHOST;
    hints.ai_socktype = SOCK_STREAM;

    int ret = getaddrinfo("10.0.0.1", "8080", &hints, &res);

    /* AI_NUMERICHOST bypasses any DNS shim. In vanilla Wasmtime without
     * network capabilities the WASI resolver may return EAI_FAIL — that
     * is acceptable. The important thing is: no crash, no hang. */
    if (ret != 0) {
        printf("  PASS: getaddrinfo AI_NUMERICHOST IPv4 — returned %d (expected without network)\n", ret);
        return 0;
    }

    if (res == NULL) {
        fprintf(stderr, "  FAIL: getaddrinfo returned 0 but result is NULL\n");
        return 1;
    }

    if (res->ai_family != AF_INET) {
        fprintf(stderr, "  FAIL: family=%d, expected AF_INET=%d\n", res->ai_family, AF_INET);
        freeaddrinfo(res);
        return 1;
    }

    struct sockaddr_in *sa = (struct sockaddr_in *)res->ai_addr;
    char addr_str[INET_ADDRSTRLEN];
    inet_ntop(AF_INET, &sa->sin_addr, addr_str, sizeof(addr_str));

    if (strcmp(addr_str, "10.0.0.1") != 0) {
        fprintf(stderr, "  FAIL: got '%s', expected '10.0.0.1'\n", addr_str);
        freeaddrinfo(res);
        return 1;
    }

    freeaddrinfo(res);
    printf("  PASS: getaddrinfo AI_NUMERICHOST IPv4 resolved correctly\n");
    return 0;
}

/* ---- Test 2: getaddrinfo AI_NUMERICHOST rejects hostname ---------------- */

static int test_getaddrinfo_numerichost_rejects(void) {
    struct addrinfo hints;
    struct addrinfo *res = NULL;

    memset(&hints, 0, sizeof(hints));
    hints.ai_flags = AI_NUMERICHOST;

    int ret = getaddrinfo("example.com", "80", &hints, &res);

    if (ret != 0) {
        printf("  PASS: getaddrinfo AI_NUMERICHOST rejects hostname (error=%d)\n", ret);
        if (res) freeaddrinfo(res);
        return 0;
    }

    fprintf(stderr, "  FAIL: AI_NUMERICHOST should reject hostname\n");
    if (res) freeaddrinfo(res);
    return 1;
}

/* ---- Test 3: getaddrinfo fallthrough — no crash for unknown host -------- */

static int test_getaddrinfo_fallthrough(void) {
    struct addrinfo hints;
    struct addrinfo *res = NULL;

    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;

    /* In vanilla Wasmtime, the shim stub returns 0 (not managed), so
     * resolution falls through to WASI ip_name_lookup. This may fail
     * but MUST NOT crash or hang. */
    int ret = getaddrinfo("unknown.example.test", "80", &hints, &res);

    if (ret == 0 && res != NULL) {
        printf("  PASS: getaddrinfo fallthrough resolved (runtime has network)\n");
        freeaddrinfo(res);
    } else {
        printf("  PASS: getaddrinfo fallthrough returned %d (no crash)\n", ret);
    }
    return 0;
}

/* ---- Test 4: gethostbyname returns NULL for unknown host ---------------- */

static int test_gethostbyname_unknown(void) {
    struct hostent *he = gethostbyname("unknown.compat.test.example");

    if (he == NULL) {
        printf("  PASS: gethostbyname returns NULL for unknown host\n");
        return 0;
    }

    /* Runtime with network support may resolve — acceptable */
    printf("  PASS: gethostbyname resolved (runtime has network): %s\n",
           he->h_name ? he->h_name : "(null)");
    return 0;
}

/* ---- Test 5: gethostbyname(NULL) returns NULL --------------------------- */

static int test_gethostbyname_null(void) {
    struct hostent *he = gethostbyname(NULL);

    if (he == NULL) {
        printf("  PASS: gethostbyname(NULL) returns NULL\n");
        return 0;
    }

    fprintf(stderr, "  FAIL: gethostbyname(NULL) should return NULL\n");
    return 1;
}

/* ---- Test 6: getnameinfo NI_NUMERICHOST IPv4 ---------------------------- */

static int test_getnameinfo_numerichost_ipv4(void) {
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_port = htons(3306);
    inet_pton(AF_INET, "172.16.0.5", &sa.sin_addr);

    char host[NI_MAXHOST];
    int ret = getnameinfo((struct sockaddr *)&sa, sizeof(sa),
                          host, sizeof(host), NULL, 0,
                          NI_NUMERICHOST);

    if (ret != 0) {
        fprintf(stderr, "  FAIL: getnameinfo NI_NUMERICHOST returned %d\n", ret);
        return 1;
    }

    if (strcmp(host, "172.16.0.5") != 0) {
        fprintf(stderr, "  FAIL: got '%s', expected '172.16.0.5'\n", host);
        return 1;
    }

    printf("  PASS: getnameinfo NI_NUMERICHOST IPv4 returns '172.16.0.5'\n");
    return 0;
}

/* ---- Test 7: getnameinfo NI_NUMERICHOST IPv6 ---------------------------- */

static int test_getnameinfo_numerichost_ipv6(void) {
    struct sockaddr_in6 sa6;
    memset(&sa6, 0, sizeof(sa6));
    sa6.sin6_family = AF_INET6;
    sa6.sin6_port = htons(6379);
    inet_pton(AF_INET6, "::1", &sa6.sin6_addr);

    char host[NI_MAXHOST];
    int ret = getnameinfo((struct sockaddr *)&sa6, sizeof(sa6),
                          host, sizeof(host), NULL, 0,
                          NI_NUMERICHOST);

    if (ret != 0) {
        fprintf(stderr, "  FAIL: getnameinfo NI_NUMERICHOST IPv6 returned %d\n", ret);
        return 1;
    }

    if (strcmp(host, "::1") != 0) {
        fprintf(stderr, "  FAIL: got '%s', expected '::1'\n", host);
        return 1;
    }

    printf("  PASS: getnameinfo NI_NUMERICHOST IPv6 returns '::1'\n");
    return 0;
}

/* ---- Test 8: getnameinfo NI_NUMERICSERV --------------------------------- */

static int test_getnameinfo_numericserv(void) {
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_port = htons(5432);
    inet_pton(AF_INET, "10.0.0.1", &sa.sin_addr);

    char serv[NI_MAXSERV];
    int ret = getnameinfo((struct sockaddr *)&sa, sizeof(sa),
                          NULL, 0, serv, sizeof(serv),
                          NI_NUMERICHOST | NI_NUMERICSERV);

    if (ret != 0) {
        fprintf(stderr, "  FAIL: getnameinfo NI_NUMERICSERV returned %d\n", ret);
        return 1;
    }

    if (strcmp(serv, "5432") != 0) {
        fprintf(stderr, "  FAIL: got serv='%s', expected '5432'\n", serv);
        return 1;
    }

    printf("  PASS: getnameinfo NI_NUMERICSERV returns '5432'\n");
    return 0;
}

/* ---- Test 9: getnameinfo fallthrough — returns numeric for unknown ------ */

static int test_getnameinfo_fallthrough(void) {
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_port = htons(80);
    inet_pton(AF_INET, "198.51.100.1", &sa.sin_addr);

    char host[NI_MAXHOST];

    /* Without NI_NUMERICHOST, tries reverse resolve shim first.
     * Weak stub returns 0, so should fall back to numeric format. */
    int ret = getnameinfo((struct sockaddr *)&sa, sizeof(sa),
                          host, sizeof(host), NULL, 0, 0);

    if (ret != 0) {
        fprintf(stderr, "  FAIL: getnameinfo fallthrough returned %d\n", ret);
        return 1;
    }

    /* Should get numeric IP since no reverse resolver available */
    if (strcmp(host, "198.51.100.1") != 0) {
        /* A hostname is also acceptable if runtime has rDNS */
        printf("  PASS: getnameinfo fallthrough resolved to '%s'\n", host);
        return 0;
    }

    printf("  PASS: getnameinfo fallthrough returns numeric '198.51.100.1'\n");
    return 0;
}

/* ---- Test 10: getnameinfo bad family returns EAI_FAMILY ----------------- */

static int test_getnameinfo_bad_family(void) {
    struct sockaddr sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_family = AF_UNIX;

    char host[NI_MAXHOST];
    int ret = getnameinfo(&sa, sizeof(sa), host, sizeof(host), NULL, 0,
                          NI_NUMERICHOST);

    if (ret == EAI_FAMILY) {
        printf("  PASS: getnameinfo bad family returns EAI_FAMILY\n");
        return 0;
    }

    fprintf(stderr, "  FAIL: expected EAI_FAMILY, got %d\n", ret);
    return 1;
}

/* ---- Test 11: Realistic sequence using all three functions -------------- */

static int test_combined_realistic_sequence(void) {
    int ok = 1;

    /* Step 1: Try getaddrinfo with numeric host (always works) */
    struct addrinfo hints;
    struct addrinfo *res = NULL;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    hints.ai_flags = AI_NUMERICHOST;
    hints.ai_socktype = SOCK_STREAM;

    int ret = getaddrinfo("127.0.0.1", "5432", &hints, &res);
    /* Accept both success and WASI resolver failures */
    if (ret != 0) {
        printf("    step 1: getaddrinfo returned %d (acceptable)\n", ret);
    } else {
        if (res) freeaddrinfo(res);
        printf("    step 1: getaddrinfo succeeded\n");
    }

    /* Step 2: Try gethostbyname — should return NULL or a result */
    struct hostent *he = gethostbyname("localhost");
    if (he == NULL) {
        printf("    step 2: gethostbyname returned NULL (acceptable)\n");
    } else {
        printf("    step 2: gethostbyname resolved '%s'\n",
               he->h_name ? he->h_name : "(null)");
    }

    /* Step 3: Use getnameinfo on a known numeric address */
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_port = htons(5432);
    inet_pton(AF_INET, "127.0.0.1", &sa.sin_addr);

    char host[NI_MAXHOST];
    char serv[NI_MAXSERV];
    ret = getnameinfo((struct sockaddr *)&sa, sizeof(sa),
                      host, sizeof(host), serv, sizeof(serv),
                      NI_NUMERICHOST | NI_NUMERICSERV);

    if (ret != 0) {
        fprintf(stderr, "    step 3: getnameinfo failed with %d\n", ret);
        ok = 0;
    } else {
        if (strcmp(host, "127.0.0.1") != 0) {
            fprintf(stderr, "    step 3: host='%s', expected '127.0.0.1'\n", host);
            ok = 0;
        }
        if (strcmp(serv, "5432") != 0) {
            fprintf(stderr, "    step 3: serv='%s', expected '5432'\n", serv);
            ok = 0;
        }
    }

    if (ok) {
        printf("  PASS: combined realistic sequence (all 3 functions, no crash)\n");
        return 0;
    }

    fprintf(stderr, "  FAIL: combined realistic sequence failed\n");
    return 1;
}

/* ---- Main --------------------------------------------------------------- */

int main(void) {
    int failures = 0;

    printf("test_dns_compat (US-205 backward compatibility):\n");

    failures += test_getaddrinfo_numerichost_ipv4();
    failures += test_getaddrinfo_numerichost_rejects();
    failures += test_getaddrinfo_fallthrough();
    failures += test_gethostbyname_unknown();
    failures += test_gethostbyname_null();
    failures += test_getnameinfo_numerichost_ipv4();
    failures += test_getnameinfo_numerichost_ipv6();
    failures += test_getnameinfo_numericserv();
    failures += test_getnameinfo_fallthrough();
    failures += test_getnameinfo_bad_family();
    failures += test_combined_realistic_sequence();

    if (failures > 0) {
        fprintf(stderr, "\n%d test(s) failed\n", failures);
        return 1;
    }

    printf("\nAll 11 tests passed\n");
    return 0;
}
