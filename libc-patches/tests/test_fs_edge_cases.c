/*
 * Edge-case tests for US-208: Verify filesystem patches with stock build and edge cases.
 *
 * WARPGRID_SHIM_REQUIRED
 *
 * Tests cover: 16-byte partial reads, 1-byte full reassembly, lseek combinations
 * (SEEK_SET, SEEK_CUR, SEEK_END), independent fd positions, graceful degradation
 * for non-existent paths, 1000 open/close cycle stress test, double close error,
 * and read-after-close error.
 *
 * Compile:
 *   clang --target=wasm32-wasip2 --sysroot=<patched-sysroot> \
 *     -o test_fs_edge_cases.wasm test_fs_edge_cases.c
 *
 * Run:
 *   wasmtime run test_fs_edge_cases.wasm
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>

/* ── Strong override of the WarpGrid filesystem shim ────────────────────── */

static const char RESOLV_CONTENT[] = "nameserver 10.0.0.1\nsearch warp.local\n";
static const char HOSTS_CONTENT[]  = "127.0.0.1 localhost\n10.0.0.5 db.prod.warp.local\n";

/*
 * Strong definition overrides the weak stub in warpgrid_fs_shim.c.
 * Returns file content for virtual paths, 0 for non-virtual paths.
 */
int __warpgrid_fs_read_virtual(const char *path,
                               unsigned char *out, int out_len) {
    const char *content = NULL;
    int content_len = 0;

    if (strcmp(path, "/etc/resolv.conf") == 0) {
        content = RESOLV_CONTENT;
        content_len = (int)sizeof(RESOLV_CONTENT) - 1; /* exclude NUL */
    } else if (strcmp(path, "/etc/hosts") == 0) {
        content = HOSTS_CONTENT;
        content_len = (int)sizeof(HOSTS_CONTENT) - 1;
    } else {
        return 0; /* Not a virtual path */
    }

    if (content_len > out_len)
        content_len = out_len;
    memcpy(out, content, content_len);
    return content_len;
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

#define ASSERT_EQ_INT(actual, expected, msg)                            \
    do {                                                                \
        int _a = (actual), _e = (expected);                             \
        if (_a != _e) {                                                 \
            printf("FAIL: %s (got %d, expected %d)\n",                  \
                   msg, _a, _e);                                        \
            return;                                                     \
        }                                                               \
    } while (0)

/* ── Tests ──────────────────────────────────────────────────────────────── */

/*
 * Test 1: Partial reads with 16-byte buffer.
 * Read /etc/hosts in 16-byte chunks, reassemble and verify content matches.
 */
static void test_partial_reads_16_byte_buffer(void) {
    TEST("partial reads with 16-byte buffer");

    int fd = open("/etc/hosts", O_RDONLY);
    ASSERT(fd >= 0, "open returned negative fd");

    char result[256];
    int total = 0;
    while (total < (int)sizeof(result) - 1) {
        ssize_t n = read(fd, result + total, 16);
        if (n == 0) break;
        ASSERT(n > 0, "read returned negative value");
        ASSERT(n <= 16, "read returned more than 16 bytes");
        total += (int)n;
    }
    result[total] = '\0';

    ASSERT(total == (int)(sizeof(HOSTS_CONTENT) - 1),
           "total bytes read mismatch");
    ASSERT(strcmp(result, HOSTS_CONTENT) == 0,
           "reassembled content mismatch after 16-byte reads");

    close(fd);
    PASS();
}

/*
 * Test 2: Partial reads with 1-byte buffer + full reassembly.
 * Read entire virtual file byte by byte, verify total matches expected content.
 */
static void test_partial_reads_1_byte_reassembly(void) {
    TEST("partial reads with 1-byte buffer + full reassembly");

    int fd = open("/etc/resolv.conf", O_RDONLY);
    ASSERT(fd >= 0, "open returned negative fd");

    char result[256];
    int total = 0;
    while (total < (int)sizeof(result) - 1) {
        ssize_t n = read(fd, result + total, 1);
        if (n == 0) break;
        ASSERT(n == 1, "1-byte read returned unexpected count");
        total += 1;
    }
    result[total] = '\0';

    ASSERT(total == (int)(sizeof(RESOLV_CONTENT) - 1),
           "total bytes read mismatch");
    ASSERT(strcmp(result, RESOLV_CONTENT) == 0,
           "content mismatch after 1-byte reads");

    close(fd);
    PASS();
}

/*
 * Test 3: lseek SEEK_SET, SEEK_CUR, SEEK_END combinations.
 * Exercise all three lseek whence values and verify cursor behavior.
 */
static void test_lseek_combinations(void) {
    TEST("lseek SEEK_SET, SEEK_CUR, SEEK_END combinations");

    int fd = open("/etc/resolv.conf", O_RDONLY);
    ASSERT(fd >= 0, "open failed");

    int content_len = (int)(sizeof(RESOLV_CONTENT) - 1);

    /* Read 5 bytes to advance cursor */
    char buf[64];
    ssize_t n = read(fd, buf, 5);
    ASSERT(n == 5, "initial read failed");
    ASSERT(memcmp(buf, "names", 5) == 0, "initial read content mismatch");

    /* SEEK_CUR: go back 3 bytes (cursor at 5, seek -3 → cursor at 2) */
    off_t pos = lseek(fd, -3, SEEK_CUR);
    ASSERT(pos == 2, "lseek SEEK_CUR -3 returned wrong position");

    /* Read 5 bytes starting from position 2 → "mese" + "r" = "meser" */
    n = read(fd, buf, 5);
    ASSERT(n == 5, "read after SEEK_CUR failed");
    ASSERT(memcmp(buf, "meser", 5) == 0, "content after SEEK_CUR mismatch");

    /* SEEK_END: seek to last 5 bytes */
    pos = lseek(fd, -5, SEEK_END);
    ASSERT(pos == content_len - 5, "lseek SEEK_END -5 returned wrong position");

    /* Read last 5 bytes */
    n = read(fd, buf, 5);
    ASSERT(n == 5, "read after SEEK_END failed");
    ASSERT(memcmp(buf, RESOLV_CONTENT + content_len - 5, 5) == 0,
           "content after SEEK_END mismatch");

    /* SEEK_SET: back to beginning */
    pos = lseek(fd, 0, SEEK_SET);
    ASSERT(pos == 0, "lseek SEEK_SET 0 returned wrong position");

    /* Read first 10 bytes from beginning */
    n = read(fd, buf, 10);
    ASSERT(n == 10, "read after SEEK_SET failed");
    ASSERT(memcmp(buf, "nameserver", 10) == 0,
           "content after SEEK_SET mismatch");

    close(fd);
    PASS();
}

/*
 * Test 4: Independent fds with independent positions.
 * Open same virtual path twice, read different amounts from each fd,
 * verify each fd maintains its own cursor position.
 */
static void test_independent_fd_positions(void) {
    TEST("independent fds with independent positions");

    int fd1 = open("/etc/hosts", O_RDONLY);
    int fd2 = open("/etc/hosts", O_RDONLY);
    ASSERT(fd1 >= 0, "first open failed");
    ASSERT(fd2 >= 0, "second open failed");
    ASSERT(fd1 != fd2, "should get different fd numbers");

    /* Read 10 bytes from fd1 */
    char buf1[32];
    ssize_t n1 = read(fd1, buf1, 10);
    ASSERT(n1 == 10, "first read from fd1 failed");
    ASSERT(memcmp(buf1, HOSTS_CONTENT, 10) == 0,
           "fd1 content mismatch");

    /* Read 5 bytes from fd2 — should start from beginning independently */
    char buf2[32];
    ssize_t n2 = read(fd2, buf2, 5);
    ASSERT(n2 == 5, "first read from fd2 failed");
    ASSERT(memcmp(buf2, HOSTS_CONTENT, 5) == 0,
           "fd2 should read from beginning independently");

    /* Continue reading from fd1 — should resume from position 10 */
    n1 = read(fd1, buf1, 5);
    ASSERT(n1 == 5, "second read from fd1 failed");
    ASSERT(memcmp(buf1, HOSTS_CONTENT + 10, 5) == 0,
           "fd1 should continue from position 10");

    /* Continue reading from fd2 — should resume from position 5 */
    n2 = read(fd2, buf2, 5);
    ASSERT(n2 == 5, "second read from fd2 failed");
    ASSERT(memcmp(buf2, HOSTS_CONTENT + 5, 5) == 0,
           "fd2 should continue from position 5");

    close(fd1);
    close(fd2);
    PASS();
}

/*
 * Test 5: Graceful degradation — fopen returns NULL with ENOENT.
 * Verify fopen of a non-existent virtual path returns NULL with ENOENT.
 */
static void test_graceful_degradation_enoent(void) {
    TEST("fopen non-existent path returns NULL with ENOENT");

    errno = 0;
    FILE *f = fopen("/tmp/nonexistent_edge_case.txt", "r");
    ASSERT(f == NULL, "fopen of non-existent path should return NULL");
    ASSERT(errno == ENOENT,
           "errno should be ENOENT for non-existent path without preopen");

    PASS();
}

/*
 * Test 6: 1000 open/close cycles — no fd leak.
 * Loop 1000 times: open virtual path, read a few bytes, close.
 * After the loop, verify the 1001st open still succeeds.
 */
static void test_1000_open_close_cycles(void) {
    TEST("1000 open/close cycles — no fd leak");

    for (int i = 0; i < 1000; i++) {
        int fd = open("/etc/hosts", O_RDONLY);
        ASSERT(fd >= 0, "open failed during stress cycle");

        char buf[8];
        ssize_t n = read(fd, buf, sizeof(buf));
        ASSERT(n > 0, "read failed during stress cycle");

        int rc = close(fd);
        ASSERT(rc == 0, "close failed during stress cycle");
    }

    /* Verify the 1001st open still works — fd pool not exhausted */
    int fd = open("/etc/hosts", O_RDONLY);
    ASSERT(fd >= 0, "1001st open failed — possible fd leak");

    char buf[64];
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    ASSERT(n > 0, "read after 1000 cycles failed");
    buf[n] = '\0';
    ASSERT(memcmp(buf, HOSTS_CONTENT, (size_t)n) == 0,
           "content mismatch after 1000 cycles");

    close(fd);
    PASS();
}

/*
 * Test 7: Double close returns error.
 * Open a virtual fd, close it, close it again — second close should fail.
 */
static void test_double_close_error(void) {
    TEST("double close returns error (EBADF)");

    int fd = open("/etc/hosts", O_RDONLY);
    ASSERT(fd >= 0, "open failed");

    int rc = close(fd);
    ASSERT(rc == 0, "first close failed");

    errno = 0;
    rc = close(fd);
    ASSERT(rc == -1, "second close should return -1");
    ASSERT(errno == EBADF, "errno should be EBADF for double close");

    PASS();
}

/*
 * Test 8: Read after close returns EBADF.
 * Open a virtual fd, close it, then read — should return -1 with EBADF.
 */
static void test_read_after_close_ebadf(void) {
    TEST("read after close returns EBADF");

    int fd = open("/etc/hosts", O_RDONLY);
    ASSERT(fd >= 0, "open failed");

    int rc = close(fd);
    ASSERT(rc == 0, "close failed");

    char buf[16];
    errno = 0;
    ssize_t n = read(fd, buf, sizeof(buf));
    ASSERT(n == -1, "read on closed fd should return -1");
    ASSERT(errno == EBADF, "errno should be EBADF for read on closed fd");

    PASS();
}

/* ── Main ───────────────────────────────────────────────────────────────── */

int main(void) {
    printf("=== US-208: Filesystem edge cases ===\n\n");

    test_partial_reads_16_byte_buffer();
    test_partial_reads_1_byte_reassembly();
    test_lseek_combinations();
    test_independent_fd_positions();
    test_graceful_degradation_enoent();
    test_1000_open_close_cycles();
    test_double_close_error();
    test_read_after_close_ebadf();

    printf("\n=== Results: %d/%d passed ===\n", tests_passed, tests_run);
    return (tests_passed == tests_run) ? 0 : 1;
}
