/*
 * TDD test for US-211: Patch close() for proxied file descriptors.
 *
 * This test provides strong overrides of __warpgrid_db_proxy_connect(),
 * __warpgrid_db_proxy_close(), __warpgrid_db_proxy_send(),
 * __warpgrid_db_proxy_recv(), and __warpgrid_fs_read_virtual()
 * to simulate the WarpGrid host runtime.
 *
 * close() on a proxied fd must:
 *   1. Call database-proxy.close(handle) to tear down the proxy connection.
 *   2. Remove the fd from the proxy tracking table.
 *   3. Fall through to close the underlying WASI socket fd.
 *
 * After close, subsequent send/recv/read/write on the fd should no longer
 * be intercepted by the proxy layer (returning -2 for fall-through to WASI,
 * which then returns EBADF).
 *
 * NOTE: Tests use fake fd numbers (2000+) and call proxy functions directly,
 * because Wasmtime 20's WASI socket() may block during socket creation.
 *
 * Compile:
 *   clang --target=wasm32-wasip2 --sysroot=<patched-sysroot> \
 *     -o test_socket_close_proxy.wasm test_socket_close_proxy.c
 *
 * Run:
 *   wasmtime run --wasm component-model=y -S preview2 test_socket_close_proxy.wasm
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

/* ── Extern declarations for proxy functions ────────────────────────────── */

extern int __warpgrid_proxy_connect(int fd, const struct sockaddr *addr,
                                     socklen_t addrlen);
extern int __warpgrid_proxy_fd_is_proxied(int fd);
extern int __warpgrid_proxy_fd_get_handle(int fd);
extern int __warpgrid_proxy_fd_remove(int fd);
extern int __warpgrid_proxy_send(int fd, const void *data, int len);
extern int __warpgrid_proxy_recv(int fd, void *buf, int max_len, int peek);
extern int __warpgrid_proxy_close(int fd);

/* ── Strong overrides of WarpGrid shim functions ────────────────────────── */

static int proxy_close_call_count = 0;
static int proxy_close_last_handle = -1;
static int proxy_close_return_value = 0;  /* can set to -1 to simulate error */

static int proxy_connect_call_count = 0;
static int proxy_send_call_count = 0;
static int proxy_recv_call_count = 0;

static int next_proxy_handle = 300;

/*
 * Strong override: database proxy connect.
 */
int __warpgrid_db_proxy_connect(const char *host, int port) {
    (void)host; (void)port;
    proxy_connect_call_count++;
    return next_proxy_handle++;
}

/*
 * Strong override: database proxy close.
 * This is the function being tested — it simulates the host runtime
 * tearing down the proxy connection.
 */
int __warpgrid_db_proxy_close(int handle) {
    proxy_close_call_count++;
    proxy_close_last_handle = handle;
    return proxy_close_return_value;
}

/*
 * Strong override: database proxy send.
 */
int __warpgrid_db_proxy_send(int handle, const void *data, int len) {
    (void)handle; (void)data;
    proxy_send_call_count++;
    return len;
}

/*
 * Strong override: database proxy recv.
 */
int __warpgrid_db_proxy_recv(int handle, void *buf, int max_len, int peek) {
    (void)handle; (void)buf; (void)max_len; (void)peek;
    proxy_recv_call_count++;
    return 0; /* EOF */
}

/*
 * Proxy config: defines which endpoints are WarpGrid proxy endpoints.
 */
static const char PROXY_CONF[] = "# WarpGrid proxy endpoints\n"
                                  "127.0.0.1:54321\n"
                                  "10.0.0.99:5432\n";

/*
 * Strong override: filesystem read virtual.
 */
int __warpgrid_fs_read_virtual(const char *path,
                               unsigned char *out, int out_len) {
    if (strcmp(path, "/etc/warpgrid/proxy.conf") == 0) {
        int len = (int)sizeof(PROXY_CONF) - 1;
        if (len > out_len) len = out_len;
        memcpy(out, PROXY_CONF, len);
        return len;
    }
    return 0;
}

/* ── Test helpers ───────────────────────────────────────────────────────── */

static int tests_run = 0;
static int tests_passed = 0;

#define TEST(name)                                                      \
    do {                                                                \
        tests_run++;                                                    \
        printf("  TEST: %s ... ", name);                                \
        fflush(stdout);                                                 \
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

static struct sockaddr_in make_addr(const char *ip, unsigned short port) {
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons(port);
    inet_pton(AF_INET, ip, &addr.sin_addr);
    return addr;
}

static int fake_fd_counter = 2000;

static int create_proxied_fd(void) {
    int fd = fake_fd_counter++;
    struct sockaddr_in proxy_addr = make_addr("127.0.0.1", 54321);
    int rc = __warpgrid_proxy_connect(fd,
                 (struct sockaddr *)&proxy_addr, sizeof(proxy_addr));
    if (rc != 0) return -1;
    return fd;
}

static void reset_counters(void) {
    proxy_close_call_count = 0;
    proxy_close_last_handle = -1;
    proxy_close_return_value = 0;
    proxy_send_call_count = 0;
    proxy_recv_call_count = 0;
}

/* ── Tests ──────────────────────────────────────────────────────────────── */

/*
 * Test 1: close() on proxied fd invokes database-proxy.close() with correct handle
 */
static void test_close_invokes_db_proxy_close(void) {
    TEST("close() on proxied fd invokes db_proxy_close with correct handle");
    reset_counters();

    int fd = create_proxied_fd();
    ASSERT(fd >= 0, "failed to create proxied fd");

    int handle = __warpgrid_proxy_fd_get_handle(fd);
    ASSERT(handle > 0, "handle should be positive");

    int prev_count = proxy_close_call_count;
    int rc = __warpgrid_proxy_close(fd);

    ASSERT(rc == 0, "proxy_close should return 0 on success");
    ASSERT(proxy_close_call_count == prev_count + 1,
           "db_proxy_close should be called exactly once");
    ASSERT(proxy_close_last_handle == handle,
           "db_proxy_close should receive the correct handle");

    PASS();
}

/*
 * Test 2: close() removes fd from proxy tracking table
 */
static void test_close_removes_from_tracking(void) {
    TEST("close() removes fd from proxy tracking table");
    reset_counters();

    int fd = create_proxied_fd();
    ASSERT(fd >= 0, "failed to create proxied fd");
    ASSERT(__warpgrid_proxy_fd_is_proxied(fd), "fd should be proxied before close");

    __warpgrid_proxy_close(fd);

    ASSERT(!__warpgrid_proxy_fd_is_proxied(fd),
           "fd should NOT be proxied after close");
    ASSERT(__warpgrid_proxy_fd_get_handle(fd) == -1,
           "handle should be -1 after close");

    PASS();
}

/*
 * Test 3: After close, send/recv return -2 (not proxied, fall through)
 */
static void test_send_recv_after_close_return_fallthrough(void) {
    TEST("after close, proxy_send/recv return -2 (fall through)");
    reset_counters();

    int fd = create_proxied_fd();
    ASSERT(fd >= 0, "failed to create proxied fd");

    /* Verify send/recv work before close */
    int rc = __warpgrid_proxy_send(fd, "test", 4);
    ASSERT(rc == 4, "proxy_send should work before close");

    __warpgrid_proxy_close(fd);

    /* After close, proxy layer should return -2 (not proxied) */
    int prev_send = proxy_send_call_count;
    int prev_recv = proxy_recv_call_count;

    rc = __warpgrid_proxy_send(fd, "test", 4);
    ASSERT(rc == -2, "proxy_send should return -2 after close");

    rc = __warpgrid_proxy_recv(fd, (char[16]){0}, 16, 0);
    ASSERT(rc == -2, "proxy_recv should return -2 after close");

    /* The db_proxy send/recv should NOT have been called */
    ASSERT(proxy_send_call_count == prev_send,
           "db_proxy_send should not be called after close");
    ASSERT(proxy_recv_call_count == prev_recv,
           "db_proxy_recv should not be called after close");

    PASS();
}

/*
 * Test 4: close() on non-proxied fd returns -2 (not proxied)
 */
static void test_close_nonproxied_returns_fallthrough(void) {
    TEST("close() on non-proxied fd returns -2");
    reset_counters();

    int fake_fd = 9998;
    ASSERT(!__warpgrid_proxy_fd_is_proxied(fake_fd), "fd should not be proxied");

    int prev_count = proxy_close_call_count;
    int rc = __warpgrid_proxy_close(fake_fd);

    ASSERT(rc == -2, "proxy_close should return -2 for non-proxied fd");
    ASSERT(proxy_close_call_count == prev_count,
           "db_proxy_close should NOT be called for non-proxied fd");

    PASS();
}

/*
 * Test 5: close() cleans up tracking even when db_proxy_close fails
 */
static void test_close_cleans_up_on_error(void) {
    TEST("close() cleans up tracking even when db_proxy_close fails");
    reset_counters();

    int fd = create_proxied_fd();
    ASSERT(fd >= 0, "failed to create proxied fd");
    ASSERT(__warpgrid_proxy_fd_is_proxied(fd), "fd should be proxied");

    /* Simulate host runtime error on close */
    proxy_close_return_value = -1;

    int rc = __warpgrid_proxy_close(fd);
    ASSERT(rc == -1, "proxy_close should return -1 on host error");

    /* Tracking should still be cleaned up */
    ASSERT(!__warpgrid_proxy_fd_is_proxied(fd),
           "fd should be removed from tracking even on error");
    ASSERT(__warpgrid_proxy_fd_get_handle(fd) == -1,
           "handle should be gone even on error");

    /* Reset for future tests */
    proxy_close_return_value = 0;

    PASS();
}

/*
 * Test 6: Double close on same fd is safe (idempotent)
 */
static void test_double_close_is_safe(void) {
    TEST("double close on proxied fd is safe");
    reset_counters();

    int fd = create_proxied_fd();
    ASSERT(fd >= 0, "failed to create proxied fd");

    /* First close succeeds */
    int rc1 = __warpgrid_proxy_close(fd);
    ASSERT(rc1 == 0, "first close should succeed");
    ASSERT(proxy_close_call_count == 1, "db_proxy_close called once");

    /* Second close returns -2 (not proxied anymore) */
    int rc2 = __warpgrid_proxy_close(fd);
    ASSERT(rc2 == -2, "second close should return -2 (already cleaned up)");
    ASSERT(proxy_close_call_count == 1,
           "db_proxy_close should NOT be called again");

    PASS();
}

/*
 * Test 7: 100 connect/send/recv/close cycles without fd leaks
 *
 * Verifies that the proxy tracking table is properly recycled and
 * doesn't leak entries across repeated open/close cycles.
 */
static void test_fd_leak_stress(void) {
    TEST("100 connect/send/recv/close cycles without fd leaks");
    reset_counters();

    for (int i = 0; i < 100; i++) {
        int fd = create_proxied_fd();
        ASSERT(fd >= 0, "failed to create proxied fd in stress loop");
        ASSERT(__warpgrid_proxy_fd_is_proxied(fd), "fd should be proxied");

        /* Send/recv cycle */
        int rc = __warpgrid_proxy_send(fd, "Q", 1);
        ASSERT(rc == 1, "proxy_send failed in stress loop");

        rc = __warpgrid_proxy_recv(fd, (char[16]){0}, 16, 0);
        ASSERT(rc >= 0, "proxy_recv failed in stress loop");

        /* Close */
        rc = __warpgrid_proxy_close(fd);
        ASSERT(rc == 0, "proxy_close failed in stress loop");
        ASSERT(!__warpgrid_proxy_fd_is_proxied(fd),
               "fd should not be proxied after close in stress loop");
    }

    /* Verify call counts */
    ASSERT(proxy_close_call_count == 100,
           "db_proxy_close should be called 100 times");

    PASS();
}

/*
 * Test 8: close() on proxied fd calls db_proxy_close exactly once
 */
static void test_close_called_exactly_once(void) {
    TEST("close() calls db_proxy_close exactly once per fd");
    reset_counters();

    /* Create multiple proxied fds */
    int fd1 = create_proxied_fd();
    int fd2 = create_proxied_fd();
    int fd3 = create_proxied_fd();
    ASSERT(fd1 >= 0 && fd2 >= 0 && fd3 >= 0, "failed to create fds");

    int h1 = __warpgrid_proxy_fd_get_handle(fd1);
    int h2 = __warpgrid_proxy_fd_get_handle(fd2);
    int h3 = __warpgrid_proxy_fd_get_handle(fd3);

    /* Close fd2 only */
    __warpgrid_proxy_close(fd2);
    ASSERT(proxy_close_call_count == 1, "exactly one close call");
    ASSERT(proxy_close_last_handle == h2, "should close fd2's handle");

    /* fd1 and fd3 should still be proxied */
    ASSERT(__warpgrid_proxy_fd_is_proxied(fd1), "fd1 should still be proxied");
    ASSERT(!__warpgrid_proxy_fd_is_proxied(fd2), "fd2 should not be proxied");
    ASSERT(__warpgrid_proxy_fd_is_proxied(fd3), "fd3 should still be proxied");

    /* Clean up remaining */
    __warpgrid_proxy_close(fd1);
    ASSERT(proxy_close_last_handle == h1, "should close fd1's handle");

    __warpgrid_proxy_close(fd3);
    ASSERT(proxy_close_last_handle == h3, "should close fd3's handle");
    ASSERT(proxy_close_call_count == 3, "three total close calls");

    PASS();
}

/*
 * Test 9: compile/link verification — all new close symbols resolve
 */
static void test_compile_link_close_symbols(void) {
    TEST("compile/link with close proxy shim symbols");
    /* If we got here, __warpgrid_db_proxy_close and __warpgrid_proxy_close
     * linked correctly from the socket shim archive member */
    PASS();
}

/*
 * Test 10: fd reuse after close — new connect on same fd number works
 */
static void test_fd_reuse_after_close(void) {
    TEST("fd reuse after close works correctly");
    reset_counters();

    int fd = fake_fd_counter++;  /* grab a specific fd number */

    /* Connect, verify proxied */
    struct sockaddr_in proxy_addr = make_addr("127.0.0.1", 54321);
    int rc = __warpgrid_proxy_connect(fd,
                 (struct sockaddr *)&proxy_addr, sizeof(proxy_addr));
    ASSERT(rc == 0, "first connect should succeed");
    int h1 = __warpgrid_proxy_fd_get_handle(fd);

    /* Close */
    rc = __warpgrid_proxy_close(fd);
    ASSERT(rc == 0, "close should succeed");
    ASSERT(!__warpgrid_proxy_fd_is_proxied(fd), "fd should not be proxied");

    /* Reuse same fd number (simulating OS fd recycling) */
    rc = __warpgrid_proxy_connect(fd,
             (struct sockaddr *)&proxy_addr, sizeof(proxy_addr));
    ASSERT(rc == 0, "second connect on same fd should succeed");

    int h2 = __warpgrid_proxy_fd_get_handle(fd);
    ASSERT(h2 > 0, "new handle should be positive");
    ASSERT(h2 != h1, "new handle should differ from old");
    ASSERT(__warpgrid_proxy_fd_is_proxied(fd), "fd should be proxied again");

    /* Final cleanup */
    __warpgrid_proxy_close(fd);

    PASS();
}

/* ── Main ───────────────────────────────────────────────────────────────── */

int main(void) {
    printf("=== US-211: Patch close() for proxied fds ===\n\n");
    fflush(stdout);

    test_close_invokes_db_proxy_close();
    test_close_removes_from_tracking();
    test_send_recv_after_close_return_fallthrough();
    test_close_nonproxied_returns_fallthrough();
    test_close_cleans_up_on_error();
    test_double_close_is_safe();
    test_fd_leak_stress();
    test_close_called_exactly_once();
    test_compile_link_close_symbols();
    test_fd_reuse_after_close();

    printf("\n=== Results: %d/%d passed ===\n", tests_passed, tests_run);
    return (tests_passed == tests_run) ? 0 : 1;
}
