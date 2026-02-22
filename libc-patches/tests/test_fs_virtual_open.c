/*
 * TDD test for US-206: Patch fopen/open to intercept virtual filesystem paths.
 *
 * This test provides a strong override of __warpgrid_fs_read_virtual() that
 * returns known content for specific virtual paths. This simulates the
 * WarpGrid host runtime providing virtual file content.
 *
 * Compile:
 *   clang --target=wasm32-wasip2 --sysroot=<patched-sysroot> \
 *     -o test_fs_virtual_open.wasm test_fs_virtual_open.c
 *
 * Run:
 *   wasmtime run test_fs_virtual_open.wasm
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

/* ── Tests ──────────────────────────────────────────────────────────────── */

/*
 * Test 1: fopen("/etc/resolv.conf", "r") returns content from shim.
 * FILE* supports fread, fgets, fclose, feof.
 */
static void test_fopen_virtual_path_read(void) {
    TEST("fopen(\"/etc/resolv.conf\", \"r\") returns shim content");

    FILE *f = fopen("/etc/resolv.conf", "r");
    ASSERT(f != NULL, "fopen returned NULL for virtual path");

    char buf[256];
    memset(buf, 0, sizeof(buf));
    size_t n = fread(buf, 1, sizeof(buf) - 1, f);
    ASSERT(n == sizeof(RESOLV_CONTENT) - 1, "fread returned wrong byte count");
    ASSERT(strcmp(buf, RESOLV_CONTENT) == 0, "fread content mismatch");

    /* feof should be set after reading all content */
    int ch = fgetc(f);
    ASSERT(ch == EOF, "expected EOF after reading all content");
    ASSERT(feof(f), "feof not set after reading all content");

    int rc = fclose(f);
    ASSERT(rc == 0, "fclose failed");

    PASS();
}

/*
 * Test 2: fgets works on virtual FILE*.
 */
static void test_fopen_fgets(void) {
    TEST("fgets on virtual FILE*");

    FILE *f = fopen("/etc/resolv.conf", "r");
    ASSERT(f != NULL, "fopen returned NULL");

    char line[128];
    char *result = fgets(line, sizeof(line), f);
    ASSERT(result != NULL, "fgets returned NULL");
    ASSERT(strcmp(line, "nameserver 10.0.0.1\n") == 0, "fgets first line mismatch");

    result = fgets(line, sizeof(line), f);
    ASSERT(result != NULL, "fgets returned NULL for second line");
    ASSERT(strcmp(line, "search warp.local\n") == 0, "fgets second line mismatch");

    /* Should be at EOF now */
    result = fgets(line, sizeof(line), f);
    ASSERT(result == NULL, "fgets should return NULL at EOF");

    fclose(f);
    PASS();
}

/*
 * Test 3: open() returns valid fd supporting read() and close().
 */
static void test_open_virtual_path_read(void) {
    TEST("open(\"/etc/resolv.conf\", O_RDONLY) + read + close");

    int fd = open("/etc/resolv.conf", O_RDONLY);
    ASSERT(fd >= 0, "open returned negative fd for virtual path");

    char buf[256];
    memset(buf, 0, sizeof(buf));
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    ASSERT(n == (ssize_t)(sizeof(RESOLV_CONTENT) - 1),
           "read returned wrong byte count");
    ASSERT(strcmp(buf, RESOLV_CONTENT) == 0, "read content mismatch");

    /* Second read should return 0 (EOF) */
    n = read(fd, buf, sizeof(buf));
    ASSERT(n == 0, "expected 0 from read at EOF");

    int rc = close(fd);
    ASSERT(rc == 0, "close failed on virtual fd");

    PASS();
}

/*
 * Test 4: Partial reads with small buffers work correctly.
 */
static void test_open_partial_reads(void) {
    TEST("partial reads with small buffer (1 byte at a time)");

    int fd = open("/etc/hosts", O_RDONLY);
    ASSERT(fd >= 0, "open returned negative fd");

    char result[256];
    int total = 0;
    while (total < (int)sizeof(result) - 1) {
        ssize_t n = read(fd, result + total, 1);
        if (n == 0) break;
        ASSERT(n == 1, "1-byte read returned unexpected count");
        total += (int)n;
    }
    result[total] = '\0';
    ASSERT(total == (int)(sizeof(HOSTS_CONTENT) - 1),
           "total bytes read mismatch");
    ASSERT(strcmp(result, HOSTS_CONTENT) == 0, "content mismatch after 1-byte reads");

    close(fd);
    PASS();
}

/*
 * Test 5: fopen with write mode on virtual path returns NULL with EROFS.
 */
static void test_fopen_write_mode_erofs(void) {
    TEST("fopen(\"/etc/resolv.conf\", \"w\") returns NULL with EROFS");

    errno = 0;
    FILE *f = fopen("/etc/resolv.conf", "w");
    ASSERT(f == NULL, "fopen(\"w\") should return NULL for virtual path");
    ASSERT(errno == EROFS, "errno should be EROFS for write on virtual path");

    PASS();
}

/*
 * Test 6: open with write flags on virtual path returns -1 with EROFS.
 */
static void test_open_write_mode_erofs(void) {
    TEST("open(\"/etc/resolv.conf\", O_WRONLY) returns -1 with EROFS");

    errno = 0;
    int fd = open("/etc/resolv.conf", O_WRONLY);
    ASSERT(fd == -1, "open(O_WRONLY) should return -1 for virtual path");
    ASSERT(errno == EROFS, "errno should be EROFS for write on virtual path");

    PASS();
}

/*
 * Test 7: Non-virtual path falls through to original WASI implementation.
 * In a vanilla Wasmtime without preopened dirs, this should fail with
 * ENOENT (path not found), not crash.
 */
static void test_nonvirtual_path_fallthrough(void) {
    TEST("non-virtual path falls through to WASI");

    errno = 0;
    FILE *f = fopen("/tmp/nonexistent_file_xyz.txt", "r");
    /* Expected: NULL because there's no preopen for /tmp */
    ASSERT(f == NULL, "fopen of non-virtual path should return NULL without preopen");
    /* errno should be ENOENT (no preopen covers this path) */
    ASSERT(errno == ENOENT, "errno should be ENOENT for non-virtual path without preopen");

    PASS();
}

/*
 * Test 8: Opening the same virtual path twice returns independent handles.
 */
static void test_independent_handles(void) {
    TEST("two independent handles to same virtual path");

    int fd1 = open("/etc/resolv.conf", O_RDONLY);
    int fd2 = open("/etc/resolv.conf", O_RDONLY);
    ASSERT(fd1 >= 0, "first open failed");
    ASSERT(fd2 >= 0, "second open failed");
    ASSERT(fd1 != fd2, "should get different fd numbers");

    /* Read 5 bytes from fd1 */
    char buf1[8];
    ssize_t n1 = read(fd1, buf1, 5);
    ASSERT(n1 == 5, "first read from fd1 failed");

    /* Read 10 bytes from fd2 — should start from beginning */
    char buf2[16];
    ssize_t n2 = read(fd2, buf2, 10);
    ASSERT(n2 == 10, "first read from fd2 failed");

    /* Verify fd2 started from beginning (independent cursor) */
    ASSERT(memcmp(buf2, RESOLV_CONTENT, 10) == 0,
           "fd2 should read from beginning independently");

    close(fd1);
    close(fd2);
    PASS();
}

/*
 * Test 9: lseek on virtual fd works.
 */
static void test_lseek_virtual_fd(void) {
    TEST("lseek on virtual fd");

    int fd = open("/etc/resolv.conf", O_RDONLY);
    ASSERT(fd >= 0, "open failed");

    /* Read 5 bytes to advance cursor */
    char buf[64];
    read(fd, buf, 5);

    /* Seek back to beginning */
    off_t pos = lseek(fd, 0, SEEK_SET);
    ASSERT(pos == 0, "lseek SEEK_SET to 0 failed");

    /* Read again should get the same content */
    memset(buf, 0, sizeof(buf));
    ssize_t n = read(fd, buf, 11);
    ASSERT(n == 11, "re-read after lseek failed");
    ASSERT(memcmp(buf, "nameserver ", 11) == 0, "content after lseek mismatch");

    /* Seek to end */
    pos = lseek(fd, 0, SEEK_END);
    ASSERT(pos == (off_t)(sizeof(RESOLV_CONTENT) - 1), "lseek SEEK_END wrong position");

    /* Read at end should return 0 */
    n = read(fd, buf, 1);
    ASSERT(n == 0, "read at end should return 0");

    close(fd);
    PASS();
}

/*
 * Test 10: Close followed by read returns error.
 */
static void test_close_then_read_error(void) {
    TEST("read after close returns error");

    int fd = open("/etc/hosts", O_RDONLY);
    ASSERT(fd >= 0, "open failed");

    int rc = close(fd);
    ASSERT(rc == 0, "close failed");

    /* read on closed virtual fd should fail */
    char buf[16];
    errno = 0;
    ssize_t n = read(fd, buf, sizeof(buf));
    ASSERT(n == -1, "read on closed fd should return -1");
    ASSERT(errno == EBADF, "errno should be EBADF for read on closed fd");

    PASS();
}

/* ── Main ───────────────────────────────────────────────────────────────── */

int main(void) {
    printf("=== US-206: Virtual filesystem fopen/open interception ===\n\n");

    test_fopen_virtual_path_read();
    test_fopen_fgets();
    test_open_virtual_path_read();
    test_open_partial_reads();
    test_fopen_write_mode_erofs();
    test_open_write_mode_erofs();
    test_nonvirtual_path_fallthrough();
    test_independent_handles();
    test_lseek_virtual_fd();
    test_close_then_read_error();

    printf("\n=== Results: %d/%d passed ===\n", tests_passed, tests_run);
    return (tests_passed == tests_run) ? 0 : 1;
}
