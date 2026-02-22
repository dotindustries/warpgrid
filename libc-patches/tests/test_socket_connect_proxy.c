/*
 * TDD test for US-209: Patch connect() to route database proxy connections.
 *
 * This test provides a strong override of __warpgrid_db_proxy_connect() and
 * __warpgrid_fs_read_virtual() to simulate the WarpGrid host runtime.
 *
 * The proxy shim intercepts connect() calls to addresses matching configured
 * proxy endpoints (read from /etc/warpgrid/proxy.conf via the FS shim) and
 * routes them through database-proxy.connect().
 *
 * Compile:
 *   clang --target=wasm32-wasip2 --sysroot=<patched-sysroot> \
 *     -o test_socket_connect_proxy.wasm test_socket_connect_proxy.c
 *
 * Run:
 *   wasmtime run --inherit-network test_socket_connect_proxy.wasm
 *
 * WARPGRID_SHIM_REQUIRED
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <unistd.h>

/* ── Strong overrides of WarpGrid shim functions ────────────────────────── */

/*
 * Track calls to the proxy connect shim for test verification.
 */
static int proxy_connect_call_count = 0;
static char last_proxy_host[64];
static int last_proxy_port = 0;
static int next_proxy_handle = 100; /* simulated handle */

/*
 * Strong override: database proxy connect.
 * Returns a positive connection handle if this host:port is proxied,
 * 0 if not proxied (fall through), -1 on error.
 */
int __warpgrid_db_proxy_connect(const char *host, int port) {
    proxy_connect_call_count++;
    strncpy(last_proxy_host, host, sizeof(last_proxy_host) - 1);
    last_proxy_host[sizeof(last_proxy_host) - 1] = '\0';
    last_proxy_port = port;
    return next_proxy_handle++;
}

/*
 * Proxy config: defines which endpoints are WarpGrid proxy endpoints.
 */
static const char PROXY_CONF[] = "# WarpGrid proxy endpoints\n"
                                  "127.0.0.1:54321\n"
                                  "10.0.0.99:5432\n";

/*
 * Strong override: filesystem read virtual.
 * Provides /etc/warpgrid/proxy.conf content.
 */
int __warpgrid_fs_read_virtual(const char *path,
                               unsigned char *out, int out_len) {
    if (strcmp(path, "/etc/warpgrid/proxy.conf") == 0) {
        int len = (int)sizeof(PROXY_CONF) - 1;
        if (len > out_len) len = out_len;
        memcpy(out, PROXY_CONF, len);
        return len;
    }
    return 0; /* Not virtual */
}

/* ── Test helpers ───────────────────────────────────────────────────────── */

static int tests_run = 0;
static int tests_passed = 0;

#define TEST(name)                                                      \
    do {                                                                \
        tests_run++;                                                    \
        printf("  TEST: %s ... ", name);                                \
    } while (0)

#define PASS()                                                          \
    do {                                                                \
        tests_passed++;                                                 \
        printf("PASS\n");                                               \
    } while (0)

#define FAIL(msg)                                                       \
    do {                                                                \
        printf("FAIL: %s\n", msg);                                      \
    } while (0)

#define ASSERT(cond, msg)                                               \
    do {                                                                \
        if (!(cond)) { FAIL(msg); return; }                             \
    } while (0)

/* Helper: create a sockaddr_in for testing */
static struct sockaddr_in make_addr(const char *ip, unsigned short port) {
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons(port);
    inet_pton(AF_INET, ip, &addr.sin_addr);
    return addr;
}

/* ── Tests ──────────────────────────────────────────────────────────────── */

/*
 * Test 1: connect() to proxy address invokes database-proxy.connect()
 * and returns 0 (success).
 */
static void test_connect_to_proxy_invokes_shim(void) {
    TEST("connect() to proxy address invokes shim");

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    ASSERT(fd >= 0, "socket() failed");

    struct sockaddr_in proxy_addr = make_addr("127.0.0.1", 54321);

    int prev_count = proxy_connect_call_count;
    int rc = connect(fd, (struct sockaddr *)&proxy_addr, sizeof(proxy_addr));
    ASSERT(rc == 0, "connect() to proxy should return 0 (success)");
    ASSERT(proxy_connect_call_count == prev_count + 1,
           "proxy connect shim not called");
    ASSERT(strcmp(last_proxy_host, "127.0.0.1") == 0,
           "wrong host passed to shim");
    ASSERT(last_proxy_port == 54321, "wrong port passed to shim");

    close(fd);
    PASS();
}

/*
 * Test 2: connect() to second proxy address also invokes shim.
 */
static void test_connect_to_second_proxy(void) {
    TEST("connect() to second configured proxy endpoint");

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    ASSERT(fd >= 0, "socket() failed");

    struct sockaddr_in proxy_addr = make_addr("10.0.0.99", 5432);

    int prev_count = proxy_connect_call_count;
    int rc = connect(fd, (struct sockaddr *)&proxy_addr, sizeof(proxy_addr));
    ASSERT(rc == 0, "connect() to proxy should return 0");
    ASSERT(proxy_connect_call_count == prev_count + 1,
           "proxy connect shim not called for second endpoint");
    ASSERT(strcmp(last_proxy_host, "10.0.0.99") == 0, "wrong host");
    ASSERT(last_proxy_port == 5432, "wrong port");

    close(fd);
    PASS();
}

/*
 * Test 3: connect() to non-proxy address falls through to WASI socket.
 * In a sandboxed Wasmtime, this will fail with ECONNREFUSED or similar,
 * but the key assertion is that the proxy shim is NOT called.
 */
static void test_connect_nonproxy_falls_through(void) {
    TEST("connect() to non-proxy address falls through");

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    ASSERT(fd >= 0, "socket() failed");

    /* 93.184.216.34:80 is a non-proxy address */
    struct sockaddr_in addr = make_addr("93.184.216.34", 80);

    int prev_count = proxy_connect_call_count;
    /* We don't care about the return value (network may not be available),
     * just that the proxy shim was NOT called. */
    connect(fd, (struct sockaddr *)&addr, sizeof(addr));
    ASSERT(proxy_connect_call_count == prev_count,
           "proxy shim should NOT be called for non-proxy address");

    close(fd);
    PASS();
}

/*
 * Test 4: Proxied fd is tracked internally (fd is marked as proxied).
 * We verify by checking that the fd was successfully connected via proxy.
 * The __warpgrid_proxy_fd_is_proxied() function is the tracking check.
 */
static void test_proxied_fd_tracked(void) {
    TEST("proxied fd is tracked internally");

    /* This function is defined in warpgrid_socket_shim.c */
    extern int __warpgrid_proxy_fd_is_proxied(int fd);

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    ASSERT(fd >= 0, "socket() failed");

    /* Before connect: not proxied */
    ASSERT(!__warpgrid_proxy_fd_is_proxied(fd),
           "fd should not be proxied before connect");

    struct sockaddr_in proxy_addr = make_addr("127.0.0.1", 54321);
    int rc = connect(fd, (struct sockaddr *)&proxy_addr, sizeof(proxy_addr));
    ASSERT(rc == 0, "connect to proxy failed");

    /* After connect to proxy: should be proxied */
    ASSERT(__warpgrid_proxy_fd_is_proxied(fd),
           "fd should be proxied after connect to proxy endpoint");

    close(fd);
    PASS();
}

/*
 * Test 5: Non-proxied fd is NOT tracked.
 */
static void test_nonproxied_fd_not_tracked(void) {
    TEST("non-proxied fd is not tracked");

    extern int __warpgrid_proxy_fd_is_proxied(int fd);

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    ASSERT(fd >= 0, "socket() failed");

    /* Connect to non-proxy address (will likely fail but that's OK) */
    struct sockaddr_in addr = make_addr("93.184.216.34", 80);
    connect(fd, (struct sockaddr *)&addr, sizeof(addr));

    /* Should NOT be tracked as proxied */
    ASSERT(!__warpgrid_proxy_fd_is_proxied(fd),
           "non-proxy fd should not be tracked");

    close(fd);
    PASS();
}

/*
 * Test 6: Compile/link verification — weak symbol fallback.
 * The fact that this test compiles and links proves that the weak
 * symbol mechanism works. Our strong overrides above replace the weak stubs.
 */
static void test_compile_link_verification(void) {
    TEST("compile/link with socket proxy shim symbols");
    /* If we got here, weak/strong symbol resolution worked */
    PASS();
}

/*
 * Test 7: Multiple proxy connections get independent tracking.
 */
static void test_multiple_proxy_connections(void) {
    TEST("multiple proxy connections tracked independently");

    extern int __warpgrid_proxy_fd_is_proxied(int fd);
    extern int __warpgrid_proxy_fd_get_handle(int fd);

    int fd1 = socket(AF_INET, SOCK_STREAM, 0);
    int fd2 = socket(AF_INET, SOCK_STREAM, 0);
    ASSERT(fd1 >= 0 && fd2 >= 0, "socket() failed");
    ASSERT(fd1 != fd2, "should get different fds");

    struct sockaddr_in proxy_addr = make_addr("127.0.0.1", 54321);

    int rc1 = connect(fd1, (struct sockaddr *)&proxy_addr, sizeof(proxy_addr));
    int rc2 = connect(fd2, (struct sockaddr *)&proxy_addr, sizeof(proxy_addr));
    ASSERT(rc1 == 0 && rc2 == 0, "both proxy connects should succeed");

    ASSERT(__warpgrid_proxy_fd_is_proxied(fd1), "fd1 should be proxied");
    ASSERT(__warpgrid_proxy_fd_is_proxied(fd2), "fd2 should be proxied");

    /* Each should have a different handle */
    int h1 = __warpgrid_proxy_fd_get_handle(fd1);
    int h2 = __warpgrid_proxy_fd_get_handle(fd2);
    ASSERT(h1 != h2, "handles should differ for independent connections");
    ASSERT(h1 > 0 && h2 > 0, "handles should be positive");

    close(fd1);
    close(fd2);
    PASS();
}

/* ── Main ───────────────────────────────────────────────────────────────── */

int main(void) {
    printf("=== US-209: Patch connect() to route database proxy connections ===\n\n");

    test_connect_to_proxy_invokes_shim();
    test_connect_to_second_proxy();
    test_connect_nonproxy_falls_through();
    test_proxied_fd_tracked();
    test_nonproxied_fd_not_tracked();
    test_compile_link_verification();
    test_multiple_proxy_connections();

    printf("\n=== Results: %d/%d passed ===\n", tests_passed, tests_run);
    return (tests_passed == tests_run) ? 0 : 1;
}
