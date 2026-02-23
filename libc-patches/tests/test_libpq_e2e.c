/*
 * TDD test for US-212: libpq end-to-end database connection through proxy.
 *
 * This test validates that PostgreSQL's libpq client library, cross-compiled
 * to wasm32-wasip2, can:
 *
 *   1. Resolve a WarpGrid service hostname via DNS shim
 *   2. Connect through the socket proxy shim
 *   3. Complete Postgres startup/auth handshake
 *   4. Execute "SELECT 1" and read the result
 *   5. Cleanly disconnect
 *
 * This is the libpq companion to test_e2e_postgres.c (which tests the raw
 * wire protocol). Here we exercise the actual libpq API: PQconnectdb,
 * PQexec, PQgetvalue, PQfinish — proving that the full driver stack works
 * end-to-end through the WarpGrid shim layer.
 *
 * Build:
 *   clang --target=wasm32-wasip2 --sysroot=<patched-sysroot> \
 *     -I<libpq-wasm>/include \
 *     -o test_libpq_e2e.wasm test_libpq_e2e.c \
 *     -L<libpq-wasm>/lib -lpq
 *
 * Run:
 *   wasmtime run --wasm component-model=y -S preview2 test_libpq_e2e.wasm
 *
 * WARPGRID_SHIM_REQUIRED
 * LIBPQ_REQUIRED
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <unistd.h>
#include <netdb.h>

#include "libpq-fe.h"

/* ── WASI POSIX compat overrides ────────────────────────────────────────── */

/*
 * WASI provides select/poll but returns ENOTSUP. libpq needs these to work
 * after connect(). We provide strong overrides here (the test .o is linked
 * before libc.a, so these win).
 */

/* select — return "1 fd ready" immediately.
 * libpq uses this after connect() to wait for socket readiness. In proxy
 * mode, the socket is always ready since proxy manages I/O. */
int __wrap_select(int nfds, void *readfds, void *writefds,
                  void *exceptfds, void *timeout) {
    (void)nfds; (void)readfds; (void)writefds;
    (void)exceptfds; (void)timeout;
    return 1;
}

/* poll — return all fds ready. */
struct __wrap_pollfd { int fd; short events; short revents; };
int __wrap_poll(struct __wrap_pollfd *fds, unsigned long nfds, int timeout) {
    (void)timeout;
    for (unsigned long i = 0; i < nfds; i++) {
        fds[i].revents = fds[i].events;
    }
    return (int)nfds;
}

/* ── Postgres wire protocol constants ───────────────────────────────────── */

#define PG_MSG_AUTH             'R'
#define PG_MSG_PARAM_STATUS     'S'
#define PG_MSG_BACKEND_KEY      'K'
#define PG_MSG_READY_FOR_QUERY  'Z'
#define PG_MSG_ROW_DESCRIPTION  'T'
#define PG_MSG_DATA_ROW         'D'
#define PG_MSG_COMMAND_COMPLETE 'C'
#define PG_MSG_ERROR_RESPONSE   'E'

#define PG_AUTH_OK              0
#define PG_PROTOCOL_3_0         0x00030000

/* ── Mock Postgres server state machine ─────────────────────────────────── */

typedef enum {
    MOCK_STATE_AWAITING_STARTUP,
    MOCK_STATE_AWAITING_QUERY,
    MOCK_STATE_QUERY_SENT,
    MOCK_STATE_TERMINATED,
    MOCK_STATE_ERROR
} MockState;

static MockState mock_state = MOCK_STATE_AWAITING_STARTUP;

/* Response buffer: filled by send handler, consumed by recv */
static unsigned char mock_response[8192];
static int mock_response_len = 0;
static int mock_response_pos = 0;

/* Tracking counters */
static int dns_resolve_call_count = 0;
static int proxy_connect_call_count = 0;
static int proxy_send_call_count = 0;
static int proxy_recv_call_count = 0;
static int proxy_close_call_count = 0;

/* Error simulation flags */
static int simulate_connect_error = 0;

/* ── Wire protocol helpers ──────────────────────────────────────────────── */

static void put_be32(unsigned char *buf, int val) {
    buf[0] = (unsigned char)((val >> 24) & 0xFF);
    buf[1] = (unsigned char)((val >> 16) & 0xFF);
    buf[2] = (unsigned char)((val >> 8) & 0xFF);
    buf[3] = (unsigned char)(val & 0xFF);
}

static int get_be32(const unsigned char *buf) {
    return ((int)buf[0] << 24) | ((int)buf[1] << 16) |
           ((int)buf[2] << 8) | (int)buf[3];
}

static void put_be16(unsigned char *buf, int val) {
    buf[0] = (unsigned char)((val >> 8) & 0xFF);
    buf[1] = (unsigned char)(val & 0xFF);
}

static int build_auth_ok(unsigned char *buf) {
    buf[0] = PG_MSG_AUTH;
    put_be32(buf + 1, 8);
    put_be32(buf + 5, PG_AUTH_OK);
    return 9;
}

static int build_param_status(unsigned char *buf, const char *name,
                              const char *value) {
    int name_len = (int)strlen(name) + 1;
    int val_len = (int)strlen(value) + 1;
    int msg_len = 4 + name_len + val_len;
    buf[0] = PG_MSG_PARAM_STATUS;
    put_be32(buf + 1, msg_len);
    memcpy(buf + 5, name, name_len);
    memcpy(buf + 5 + name_len, value, val_len);
    return 1 + msg_len;
}

static int build_backend_key(unsigned char *buf) {
    buf[0] = PG_MSG_BACKEND_KEY;
    put_be32(buf + 1, 12);
    put_be32(buf + 5, 12345);
    put_be32(buf + 9, 67890);
    return 13;
}

static int build_ready_for_query(unsigned char *buf, char status) {
    buf[0] = PG_MSG_READY_FOR_QUERY;
    put_be32(buf + 1, 5);
    buf[5] = (unsigned char)status;
    return 6;
}

static int build_row_description_int(unsigned char *buf) {
    const char *col_name = "?column?";
    int name_len = (int)strlen(col_name) + 1;
    int field_len = name_len + 18;
    int msg_len = 4 + 2 + field_len;
    int pos = 0;
    buf[pos++] = PG_MSG_ROW_DESCRIPTION;
    put_be32(buf + pos, msg_len); pos += 4;
    put_be16(buf + pos, 1); pos += 2;
    memcpy(buf + pos, col_name, name_len); pos += name_len;
    put_be32(buf + pos, 0); pos += 4;    /* table OID */
    put_be16(buf + pos, 0); pos += 2;    /* column number */
    put_be32(buf + pos, 23); pos += 4;   /* type OID: int4 */
    put_be16(buf + pos, 4); pos += 2;    /* type size */
    put_be32(buf + pos, -1); pos += 4;   /* type modifier */
    put_be16(buf + pos, 0); pos += 2;    /* format: text */
    return pos;
}

static int build_data_row(unsigned char *buf, const char *value) {
    int val_len = (int)strlen(value);
    int msg_len = 4 + 2 + 4 + val_len;
    int pos = 0;
    buf[pos++] = PG_MSG_DATA_ROW;
    put_be32(buf + pos, msg_len); pos += 4;
    put_be16(buf + pos, 1); pos += 2;
    put_be32(buf + pos, val_len); pos += 4;
    memcpy(buf + pos, value, val_len); pos += val_len;
    return pos;
}

static int build_command_complete(unsigned char *buf, const char *tag) {
    int tag_len = (int)strlen(tag) + 1;
    int msg_len = 4 + tag_len;
    buf[0] = PG_MSG_COMMAND_COMPLETE;
    put_be32(buf + 1, msg_len);
    memcpy(buf + 5, tag, tag_len);
    return 1 + msg_len;
}

static int build_error_response(unsigned char *buf, const char *severity,
                                 const char *code, const char *message) {
    int pos = 5;
    buf[0] = PG_MSG_ERROR_RESPONSE;

    buf[pos++] = 'S';
    int sev_len = (int)strlen(severity) + 1;
    memcpy(buf + pos, severity, sev_len); pos += sev_len;

    buf[pos++] = 'V';  /* non-localized severity — libpq looks for this */
    memcpy(buf + pos, severity, sev_len); pos += sev_len;

    buf[pos++] = 'C';
    int code_len = (int)strlen(code) + 1;
    memcpy(buf + pos, code, code_len); pos += code_len;

    buf[pos++] = 'M';
    int msg_len = (int)strlen(message) + 1;
    memcpy(buf + pos, message, msg_len); pos += msg_len;

    buf[pos++] = '\0';

    put_be32(buf + 1, pos - 1);
    return pos;
}

/* ── Mock server response builders ──────────────────────────────────────── */

/*
 * Build the startup response that libpq expects.
 * libpq requires: AuthOk, ParameterStatus messages (at minimum
 * server_version, server_encoding, client_encoding, is_superuser,
 * session_authorization, DateStyle, IntervalStyle, TimeZone,
 * integer_datetimes, standard_conforming_strings), BackendKeyData,
 * ReadyForQuery.
 */
static void build_startup_response(void) {
    int pos = 0;

    pos += build_auth_ok(mock_response + pos);

    /* libpq reads and stores these parameter status messages */
    pos += build_param_status(mock_response + pos, "server_version", "16.2");
    pos += build_param_status(mock_response + pos, "server_encoding", "UTF8");
    pos += build_param_status(mock_response + pos, "client_encoding", "UTF8");
    pos += build_param_status(mock_response + pos, "is_superuser", "on");
    pos += build_param_status(mock_response + pos, "session_authorization", "test");
    pos += build_param_status(mock_response + pos, "DateStyle", "ISO, MDY");
    pos += build_param_status(mock_response + pos, "IntervalStyle", "postgres");
    pos += build_param_status(mock_response + pos, "TimeZone", "UTC");
    pos += build_param_status(mock_response + pos, "integer_datetimes", "on");
    pos += build_param_status(mock_response + pos, "standard_conforming_strings", "on");

    pos += build_backend_key(mock_response + pos);
    pos += build_ready_for_query(mock_response + pos, 'I');

    mock_response_len = pos;
    mock_response_pos = 0;
    mock_state = MOCK_STATE_AWAITING_QUERY;
}

static void build_query_response(void) {
    int pos = 0;
    pos += build_row_description_int(mock_response + pos);
    pos += build_data_row(mock_response + pos, "1");
    pos += build_command_complete(mock_response + pos, "SELECT 1");
    pos += build_ready_for_query(mock_response + pos, 'I');
    mock_response_len = pos;
    mock_response_pos = 0;
    mock_state = MOCK_STATE_QUERY_SENT;
}

/* ── Strong overrides of WarpGrid shim functions ────────────────────────── */

static int next_proxy_handle = 500;

int __warpgrid_dns_resolve(const char *hostname, int family,
                           unsigned char *out, int out_len) {
    dns_resolve_call_count++;
    (void)family;

    /* Resolve known WarpGrid service names and also localhost/127.0.0.1 */
    if (strcmp(hostname, "db.production.warp.local") == 0 ||
        strcmp(hostname, "127.0.0.1") == 0 ||
        strcmp(hostname, "localhost") == 0) {
        if (out_len < 17) return 0;
        out[0] = 4;  /* AF_INET */
        out[1] = 127; out[2] = 0; out[3] = 0; out[4] = 1;
        memset(out + 5, 0, 12);
        return 1;
    }

    return 0;
}

int __warpgrid_db_proxy_connect(const char *host, int port) {
    proxy_connect_call_count++;
    (void)host; (void)port;

    if (simulate_connect_error) {
        return -1;
    }

    mock_state = MOCK_STATE_AWAITING_STARTUP;
    mock_response_len = 0;
    mock_response_pos = 0;
    return next_proxy_handle++;
}

int __warpgrid_db_proxy_send(int handle, const void *data, int len) {
    (void)handle;
    proxy_send_call_count++;

    const unsigned char *msg = (const unsigned char *)data;

    switch (mock_state) {
    case MOCK_STATE_AWAITING_STARTUP:
        /* StartupMessage — no type byte, starts with length */
        build_startup_response();
        break;

    case MOCK_STATE_AWAITING_QUERY:
    case MOCK_STATE_QUERY_SENT:
        if (len > 0 && msg[0] == 'Q') {
            build_query_response();
        } else if (len > 0 && msg[0] == 'X') {
            mock_state = MOCK_STATE_TERMINATED;
            mock_response_len = 0;
        }
        break;

    default:
        break;
    }

    return len;
}

int __warpgrid_db_proxy_recv(int handle, void *buf, int max_len, int peek) {
    (void)handle;
    proxy_recv_call_count++;

    int avail = mock_response_len - mock_response_pos;
    if (avail <= 0) return 0;

    int to_copy = (max_len < avail) ? max_len : avail;
    memcpy(buf, mock_response + mock_response_pos, to_copy);

    if (!peek)
        mock_response_pos += to_copy;

    return to_copy;
}

int __warpgrid_db_proxy_close(int handle) {
    (void)handle;
    proxy_close_call_count++;
    return 0;
}

/* Proxy config — makes 127.0.0.1:5432 a proxied endpoint */
static const char PROXY_CONF[] = "# WarpGrid proxy endpoints\n"
                                  "127.0.0.1:5432\n";

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

/* ── Test framework ────────────────────────────────────────────────────── */

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

static void reset_all(void) {
    mock_state = MOCK_STATE_AWAITING_STARTUP;
    mock_response_len = 0;
    mock_response_pos = 0;
    dns_resolve_call_count = 0;
    proxy_connect_call_count = 0;
    proxy_send_call_count = 0;
    proxy_recv_call_count = 0;
    proxy_close_call_count = 0;
    next_proxy_handle = 500;
    simulate_connect_error = 0;
}

/* ── Tests ─────────────────────────────────────────────────────────────── */

/*
 * Test 1: PQconnectdb succeeds and returns CONNECTION_OK.
 * Validates: DNS → connect → startup → auth → ReadyForQuery.
 */
static void test_libpq_connect(void) {
    TEST("PQconnectdb establishes connection through proxy");
    reset_all();

    PGconn *conn = PQconnectdb(
        "host=127.0.0.1 port=5432 dbname=testdb user=test "
        "connect_timeout=5");

    ASSERT(conn != NULL, "PQconnectdb returned NULL");
    ASSERT(PQstatus(conn) == CONNECTION_OK,
           PQerrorMessage(conn));

    /* Verify proxy shims were invoked */
    ASSERT(proxy_connect_call_count > 0, "proxy connect not called");
    ASSERT(proxy_send_call_count > 0, "proxy send not called (startup)");
    ASSERT(proxy_recv_call_count > 0, "proxy recv not called (auth response)");

    PQfinish(conn);
    PASS();
}

/*
 * Test 2: PQexec("SELECT 1") returns the correct result.
 * Validates full query round-trip through proxy.
 */
static void test_libpq_select_1(void) {
    TEST("PQexec SELECT 1 returns correct result through proxy");
    reset_all();

    PGconn *conn = PQconnectdb(
        "host=127.0.0.1 port=5432 dbname=testdb user=test "
        "connect_timeout=5");
    ASSERT(conn != NULL && PQstatus(conn) == CONNECTION_OK,
           "connection failed");

    PGresult *res = PQexec(conn, "SELECT 1");
    ASSERT(res != NULL, "PQexec returned NULL");
    ASSERT(PQresultStatus(res) == PGRES_TUPLES_OK,
           PQresultErrorMessage(res));

    /* Verify result shape */
    ASSERT(PQntuples(res) == 1, "expected 1 row");
    ASSERT(PQnfields(res) == 1, "expected 1 column");

    /* Verify value */
    char *val = PQgetvalue(res, 0, 0);
    ASSERT(val != NULL, "PQgetvalue returned NULL");
    ASSERT(strcmp(val, "1") == 0, "expected value '1'");

    PQclear(res);
    PQfinish(conn);
    PASS();
}

/*
 * Test 3: PQfinish triggers proxy close.
 * Validates that disconnect goes through close shim.
 */
static void test_libpq_disconnect(void) {
    TEST("PQfinish triggers proxy close");
    reset_all();

    PGconn *conn = PQconnectdb(
        "host=127.0.0.1 port=5432 dbname=testdb user=test "
        "connect_timeout=5");
    ASSERT(conn != NULL && PQstatus(conn) == CONNECTION_OK,
           "connection failed");

    int close_before = proxy_close_call_count;
    PQfinish(conn);

    ASSERT(proxy_close_call_count > close_before,
           "proxy close not called after PQfinish");
    PASS();
}

/*
 * Test 4: Connection failure propagates as error, not crash.
 * Validates: error codes from proxy → CONNECTION_BAD status.
 */
static void test_libpq_connect_error(void) {
    TEST("connection failure returns CONNECTION_BAD, not crash");
    reset_all();
    simulate_connect_error = 1;

    PGconn *conn = PQconnectdb(
        "host=127.0.0.1 port=5432 dbname=testdb user=test "
        "connect_timeout=5");

    ASSERT(conn != NULL, "PQconnectdb returned NULL on error");
    ASSERT(PQstatus(conn) == CONNECTION_BAD,
           "expected CONNECTION_BAD on connect failure");

    /* Error message should be populated */
    const char *errmsg = PQerrorMessage(conn);
    ASSERT(errmsg != NULL && strlen(errmsg) > 0,
           "error message should be populated");

    PQfinish(conn);
    PASS();
}

/*
 * Test 5: Full lifecycle call count verification.
 * Validates all shim layers are invoked during connect → query → disconnect.
 */
static void test_libpq_full_lifecycle_counts(void) {
    TEST("full lifecycle invokes all proxy shim layers");
    reset_all();

    PGconn *conn = PQconnectdb(
        "host=127.0.0.1 port=5432 dbname=testdb user=test "
        "connect_timeout=5");
    ASSERT(conn != NULL && PQstatus(conn) == CONNECTION_OK,
           "connection failed");

    PGresult *res = PQexec(conn, "SELECT 1");
    ASSERT(res != NULL && PQresultStatus(res) == PGRES_TUPLES_OK,
           "query failed");
    PQclear(res);

    PQfinish(conn);

    /* Verify all proxy layers were used */
    ASSERT(proxy_connect_call_count > 0, "connect shim not called");
    ASSERT(proxy_send_call_count >= 2,   "send shim called < 2 times (startup + query)");
    ASSERT(proxy_recv_call_count >= 2,   "recv shim called < 2 times");
    ASSERT(proxy_close_call_count > 0,   "close shim not called");

    PASS();
}

/*
 * Test 6: libpq server version detection.
 * Validates that ParameterStatus messages are parsed correctly.
 */
static void test_libpq_server_version(void) {
    TEST("libpq detects server version from ParameterStatus");
    reset_all();

    PGconn *conn = PQconnectdb(
        "host=127.0.0.1 port=5432 dbname=testdb user=test "
        "connect_timeout=5");
    ASSERT(conn != NULL && PQstatus(conn) == CONNECTION_OK,
           "connection failed");

    int ver = PQserverVersion(conn);
    ASSERT(ver > 0, "server version should be > 0");
    /* 16.2 → 160002 */
    ASSERT(ver == 160002, "expected server version 160002 (16.2)");

    PQfinish(conn);
    PASS();
}

/* ── Main ──────────────────────────────────────────────────────────────── */

int main(void) {
    printf("=== US-212: libpq end-to-end through WarpGrid proxy ===\n");

    test_libpq_connect();
    test_libpq_select_1();
    test_libpq_disconnect();
    test_libpq_connect_error();
    test_libpq_full_lifecycle_counts();
    test_libpq_server_version();

    printf("\n%d/%d tests passed\n", tests_passed, tests_run);

    return (tests_passed == tests_run) ? 0 : 1;
}
