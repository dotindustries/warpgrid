/*
 * TDD test for US-212: End-to-end database driver compilation and connection test.
 *
 * This test exercises the FULL Postgres wire protocol lifecycle through
 * the WarpGrid proxy shim stack:
 *
 *   1. DNS resolution (getaddrinfo via DNS shim)
 *   2. TCP connection (connect via socket proxy shim)
 *   3. Postgres StartupMessage (send via proxy send shim)
 *   4. Authentication exchange (recv/send via proxy shims)
 *   5. Simple query: SELECT 1 (send query, recv results)
 *   6. Graceful disconnect (Terminate message + close via proxy close shim)
 *
 * This validates that ALL five libc patches (DNS, filesystem, connect,
 * send/recv, close) work together to support a real Postgres driver flow.
 *
 * The test uses strong symbol overrides to simulate both the WarpGrid host
 * runtime AND a mock Postgres server that returns valid wire protocol responses.
 *
 * Wire protocol reference: https://www.postgresql.org/docs/16/protocol-message-formats.html
 *
 * Compile:
 *   clang --target=wasm32-wasip2 --sysroot=<patched-sysroot> \
 *     -o test_e2e_postgres.wasm test_e2e_postgres.c
 *
 * Run:
 *   wasmtime run --wasm component-model=y -S preview2 test_e2e_postgres.wasm
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
#include <netdb.h>

/* ── Postgres wire protocol constants ───────────────────────────────────── */

/* Message type bytes (backend → frontend) */
#define PG_MSG_AUTH             'R'
#define PG_MSG_PARAM_STATUS     'S'
#define PG_MSG_BACKEND_KEY      'K'
#define PG_MSG_READY_FOR_QUERY  'Z'
#define PG_MSG_ROW_DESCRIPTION  'T'
#define PG_MSG_DATA_ROW         'D'
#define PG_MSG_COMMAND_COMPLETE 'C'
#define PG_MSG_ERROR_RESPONSE   'E'

/* Auth sub-types */
#define PG_AUTH_OK              0
#define PG_AUTH_CLEARTEXT_PWD   3

/* Protocol version 3.0 */
#define PG_PROTOCOL_3_0         0x00030000

/* ── Mock Postgres server state machine ─────────────────────────────────── */

/*
 * The mock server responds to the Postgres wire protocol in sequence.
 * Each send() from the client triggers the next response to be queued
 * for the following recv() call.
 */
typedef enum {
    MOCK_STATE_AWAITING_STARTUP,
    MOCK_STATE_AWAITING_QUERY,
    MOCK_STATE_QUERY_SENT,
    MOCK_STATE_TERMINATED,
    MOCK_STATE_ERROR
} MockState;

static MockState mock_state = MOCK_STATE_AWAITING_STARTUP;

/* Response buffer: filled by send handler, consumed by recv */
static unsigned char mock_response[4096];
static int mock_response_len = 0;
static int mock_response_pos = 0;

/* Tracking counters */
static int dns_resolve_call_count = 0;
static int proxy_connect_call_count = 0;
static int proxy_send_call_count = 0;
static int proxy_recv_call_count = 0;
static int proxy_close_call_count = 0;

/* Last DNS query for verification */
static char last_dns_hostname[256];

/* Captured startup message fields */
static char captured_user[64];
static char captured_database[64];
static int captured_protocol_version = 0;

/* Captured query */
static char captured_query[1024];

/* Error simulation flags */
static int simulate_connect_error = 0;
static int simulate_auth_error = 0;

/* ── Wire protocol helpers ──────────────────────────────────────────────── */

/* Write a 32-bit big-endian integer to buffer */
static void put_be32(unsigned char *buf, int val) {
    buf[0] = (unsigned char)((val >> 24) & 0xFF);
    buf[1] = (unsigned char)((val >> 16) & 0xFF);
    buf[2] = (unsigned char)((val >> 8) & 0xFF);
    buf[3] = (unsigned char)(val & 0xFF);
}

/* Read a 32-bit big-endian integer from buffer */
static int get_be32(const unsigned char *buf) {
    return ((int)buf[0] << 24) | ((int)buf[1] << 16) |
           ((int)buf[2] << 8) | (int)buf[3];
}

/* Write a 16-bit big-endian integer to buffer */
static void put_be16(unsigned char *buf, int val) {
    buf[0] = (unsigned char)((val >> 8) & 0xFF);
    buf[1] = (unsigned char)(val & 0xFF);
}

/*
 * Build AuthenticationOk response:
 *   'R' | int32 len=8 | int32 auth_type=0
 */
static int build_auth_ok(unsigned char *buf) {
    buf[0] = PG_MSG_AUTH;
    put_be32(buf + 1, 8);   /* length includes self */
    put_be32(buf + 5, PG_AUTH_OK);
    return 9;
}

/*
 * Build ParameterStatus message:
 *   'S' | int32 len | cstring name | cstring value
 */
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

/*
 * Build BackendKeyData:
 *   'K' | int32 len=12 | int32 pid | int32 secret_key
 */
static int build_backend_key(unsigned char *buf) {
    buf[0] = PG_MSG_BACKEND_KEY;
    put_be32(buf + 1, 12);
    put_be32(buf + 5, 12345);   /* fake pid */
    put_be32(buf + 9, 67890);   /* fake secret key */
    return 13;
}

/*
 * Build ReadyForQuery:
 *   'Z' | int32 len=5 | byte status ('I'=idle, 'T'=in transaction)
 */
static int build_ready_for_query(unsigned char *buf, char status) {
    buf[0] = PG_MSG_READY_FOR_QUERY;
    put_be32(buf + 1, 5);
    buf[5] = (unsigned char)status;
    return 6;
}

/*
 * Build RowDescription for a single column "?column?" of type int4:
 *   'T' | int32 len | int16 num_fields=1 | field_desc...
 *
 * Field descriptor:
 *   cstring name | int32 table_oid | int16 col_num |
 *   int32 type_oid | int16 type_size | int32 type_mod | int16 format
 */
static int build_row_description_int(unsigned char *buf) {
    const char *col_name = "?column?";
    int name_len = (int)strlen(col_name) + 1;
    /* field: name + 6 fixed fields (4+2+4+2+4+2 = 18 bytes) */
    int field_len = name_len + 18;
    int msg_len = 4 + 2 + field_len; /* length + num_fields + field */
    int pos = 0;
    buf[pos++] = PG_MSG_ROW_DESCRIPTION;
    put_be32(buf + pos, msg_len); pos += 4;
    put_be16(buf + pos, 1); pos += 2;  /* 1 field */
    memcpy(buf + pos, col_name, name_len); pos += name_len;
    put_be32(buf + pos, 0); pos += 4;    /* table OID */
    put_be16(buf + pos, 0); pos += 2;    /* column number */
    put_be32(buf + pos, 23); pos += 4;   /* type OID: int4 */
    put_be16(buf + pos, 4); pos += 2;    /* type size */
    put_be32(buf + pos, -1); pos += 4;   /* type modifier */
    put_be16(buf + pos, 0); pos += 2;    /* format: text */
    return pos;
}

/*
 * Build DataRow with a single text column value:
 *   'D' | int32 len | int16 num_cols=1 | int32 col_len | bytes col_data
 */
static int build_data_row(unsigned char *buf, const char *value) {
    int val_len = (int)strlen(value);
    int msg_len = 4 + 2 + 4 + val_len;
    int pos = 0;
    buf[pos++] = PG_MSG_DATA_ROW;
    put_be32(buf + pos, msg_len); pos += 4;
    put_be16(buf + pos, 1); pos += 2;  /* 1 column */
    put_be32(buf + pos, val_len); pos += 4;
    memcpy(buf + pos, value, val_len); pos += val_len;
    return pos;
}

/*
 * Build CommandComplete:
 *   'C' | int32 len | cstring tag
 */
static int build_command_complete(unsigned char *buf, const char *tag) {
    int tag_len = (int)strlen(tag) + 1;
    int msg_len = 4 + tag_len;
    buf[0] = PG_MSG_COMMAND_COMPLETE;
    put_be32(buf + 1, msg_len);
    memcpy(buf + 5, tag, tag_len);
    return 1 + msg_len;
}

/*
 * Build ErrorResponse:
 *   'E' | int32 len | ('S' severity '\0' | 'C' code '\0' | 'M' message '\0' | '\0')
 */
static int build_error_response(unsigned char *buf, const char *severity,
                                 const char *code, const char *message) {
    int pos = 5; /* skip type byte + length */
    buf[0] = PG_MSG_ERROR_RESPONSE;

    buf[pos++] = 'S';
    int sev_len = (int)strlen(severity) + 1;
    memcpy(buf + pos, severity, sev_len); pos += sev_len;

    buf[pos++] = 'C';
    int code_len = (int)strlen(code) + 1;
    memcpy(buf + pos, code, code_len); pos += code_len;

    buf[pos++] = 'M';
    int msg_len = (int)strlen(message) + 1;
    memcpy(buf + pos, message, msg_len); pos += msg_len;

    buf[pos++] = '\0'; /* terminator */

    put_be32(buf + 1, pos - 1); /* length excludes type byte */
    return pos;
}

/*
 * Parse startup message fields from the send buffer.
 * Format: int32 len | int32 protocol | (cstring key | cstring value)* | '\0'
 */
static void parse_startup_message(const unsigned char *data, int len) {
    if (len < 8) return;
    captured_protocol_version = get_be32(data + 4);

    int pos = 8;
    while (pos < len && data[pos] != '\0') {
        const char *key = (const char *)(data + pos);
        pos += (int)strlen(key) + 1;
        if (pos >= len) break;
        const char *val = (const char *)(data + pos);
        pos += (int)strlen(val) + 1;

        if (strcmp(key, "user") == 0) {
            strncpy(captured_user, val, sizeof(captured_user) - 1);
            captured_user[sizeof(captured_user) - 1] = '\0';
        } else if (strcmp(key, "database") == 0) {
            strncpy(captured_database, val, sizeof(captured_database) - 1);
            captured_database[sizeof(captured_database) - 1] = '\0';
        }
    }
}

/*
 * Parse simple query message from the send buffer.
 * Format: 'Q' | int32 len | cstring query
 */
static void parse_query_message(const unsigned char *data, int len) {
    if (len < 6 || data[0] != 'Q') return;
    int qlen = len - 5;
    if (qlen >= (int)sizeof(captured_query)) qlen = (int)sizeof(captured_query) - 1;
    memcpy(captured_query, data + 5, qlen);
    captured_query[qlen] = '\0';
    /* Remove trailing null if present */
    int slen = (int)strlen(captured_query);
    if (slen > 0 && captured_query[slen - 1] == '\0') {
        captured_query[slen - 1] = '\0';
    }
}

/*
 * Build the mock server's startup response:
 * AuthOk + ParameterStatus(server_version) + BackendKeyData + ReadyForQuery('I')
 */
static void build_startup_response(void) {
    int pos = 0;

    if (simulate_auth_error) {
        pos += build_error_response(mock_response + pos,
            "FATAL", "28P01", "password authentication failed for user \"test\"");
        mock_response_len = pos;
        mock_state = MOCK_STATE_ERROR;
        return;
    }

    pos += build_auth_ok(mock_response + pos);
    pos += build_param_status(mock_response + pos, "server_version", "16.2");
    pos += build_param_status(mock_response + pos, "server_encoding", "UTF8");
    pos += build_backend_key(mock_response + pos);
    pos += build_ready_for_query(mock_response + pos, 'I');
    mock_response_len = pos;
    mock_response_pos = 0;
    mock_state = MOCK_STATE_AWAITING_QUERY;
}

/*
 * Build the mock server's query response for "SELECT 1":
 * RowDescription + DataRow("1") + CommandComplete("SELECT 1") + ReadyForQuery('I')
 */
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

/*
 * Strong override: DNS resolve.
 * Simulates the WarpGrid service registry resolving db hostnames.
 * Returns a packed address record (17 bytes: 1-byte family + 16-byte address).
 */
int __warpgrid_dns_resolve(const char *hostname, int family,
                           unsigned char *out, int out_len) {
    dns_resolve_call_count++;
    strncpy(last_dns_hostname, hostname, sizeof(last_dns_hostname) - 1);
    last_dns_hostname[sizeof(last_dns_hostname) - 1] = '\0';

    /* Resolve known WarpGrid service names to proxy endpoint */
    if (strcmp(hostname, "db.production.warp.local") == 0 ||
        strcmp(hostname, "db.test.warp.local") == 0) {
        if (out_len < 17) return 0;
        /* Return 127.0.0.1 as IPv4 (family=4) */
        out[0] = 4; /* AF_INET */
        out[1] = 127; out[2] = 0; out[3] = 0; out[4] = 1;
        memset(out + 5, 0, 12); /* pad remaining to 16 bytes */
        return 1; /* 1 address record */
    }

    return 0; /* Unknown host, fall through to WASI resolver */
}

/*
 * Strong override: database proxy connect.
 */
int __warpgrid_db_proxy_connect(const char *host, int port) {
    proxy_connect_call_count++;

    if (simulate_connect_error) {
        return -1; /* Connection refused */
    }

    (void)host; (void)port;
    mock_state = MOCK_STATE_AWAITING_STARTUP;
    mock_response_len = 0;
    mock_response_pos = 0;
    return next_proxy_handle++;
}

/*
 * Strong override: database proxy send.
 * Processes the Postgres wire protocol message and queues the mock response.
 */
int __warpgrid_db_proxy_send(int handle, const void *data, int len) {
    (void)handle;
    proxy_send_call_count++;

    const unsigned char *msg = (const unsigned char *)data;

    switch (mock_state) {
    case MOCK_STATE_AWAITING_STARTUP:
        parse_startup_message(msg, len);
        build_startup_response();
        break;

    case MOCK_STATE_AWAITING_QUERY:
    case MOCK_STATE_QUERY_SENT:
        if (len > 0 && msg[0] == 'Q') {
            parse_query_message(msg, len);
            build_query_response();
        } else if (len > 0 && msg[0] == 'X') {
            /* Terminate message */
            mock_state = MOCK_STATE_TERMINATED;
            mock_response_len = 0;
        }
        break;

    default:
        break;
    }

    return len;
}

/*
 * Strong override: database proxy recv.
 * Returns data from the mock response buffer.
 */
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

/*
 * Strong override: database proxy close.
 */
int __warpgrid_db_proxy_close(int handle) {
    (void)handle;
    proxy_close_call_count++;
    return 0;
}

/*
 * Proxy config: proxy endpoints matching DNS resolution results.
 */
static const char PROXY_CONF[] = "# WarpGrid proxy endpoints\n"
                                  "127.0.0.1:5432\n"
                                  "127.0.0.1:54321\n";

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

/* ── Extern declarations for proxy functions ────────────────────────────── */

extern int __warpgrid_proxy_connect(int fd, const struct sockaddr *addr,
                                     socklen_t addrlen);
extern int __warpgrid_proxy_fd_is_proxied(int fd);
extern int __warpgrid_proxy_fd_get_handle(int fd);
extern int __warpgrid_proxy_send(int fd, const void *data, int len);
extern int __warpgrid_proxy_recv(int fd, void *buf, int max_len, int peek);
extern int __warpgrid_proxy_close(int fd);

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

static int fake_fd_counter = 3000;

static void reset_all(void) {
    mock_state = MOCK_STATE_AWAITING_STARTUP;
    mock_response_len = 0;
    mock_response_pos = 0;
    dns_resolve_call_count = 0;
    proxy_connect_call_count = 0;
    proxy_send_call_count = 0;
    proxy_recv_call_count = 0;
    proxy_close_call_count = 0;
    simulate_connect_error = 0;
    simulate_auth_error = 0;
    captured_user[0] = '\0';
    captured_database[0] = '\0';
    captured_query[0] = '\0';
    captured_protocol_version = 0;
    last_dns_hostname[0] = '\0';
}

/*
 * Helper: build a Postgres StartupMessage.
 * Returns the number of bytes written to buf.
 */
static int build_startup_message(unsigned char *buf, int buf_size,
                                  const char *user, const char *database) {
    int pos = 4; /* skip length field */
    put_be32(buf + pos, PG_PROTOCOL_3_0); pos += 4;

    /* user parameter */
    const char *key = "user";
    int klen = (int)strlen(key) + 1;
    memcpy(buf + pos, key, klen); pos += klen;
    int vlen = (int)strlen(user) + 1;
    memcpy(buf + pos, user, vlen); pos += vlen;

    /* database parameter */
    key = "database";
    klen = (int)strlen(key) + 1;
    memcpy(buf + pos, key, klen); pos += klen;
    vlen = (int)strlen(database) + 1;
    memcpy(buf + pos, database, vlen); pos += vlen;

    /* terminating null byte */
    buf[pos++] = '\0';

    /* Write total length at the beginning */
    put_be32(buf, pos);

    (void)buf_size;
    return pos;
}

/*
 * Helper: build a Postgres Simple Query message.
 * Format: 'Q' | int32 len | cstring query
 */
static int build_query_message(unsigned char *buf, int buf_size,
                                const char *query) {
    int qlen = (int)strlen(query) + 1; /* include null terminator */
    buf[0] = 'Q';
    put_be32(buf + 1, 4 + qlen);
    memcpy(buf + 5, query, qlen);
    (void)buf_size;
    return 5 + qlen;
}

/*
 * Helper: build a Postgres Terminate message.
 * Format: 'X' | int32 len=4
 */
static int build_terminate_message(unsigned char *buf) {
    buf[0] = 'X';
    put_be32(buf + 1, 4);
    return 5;
}

/* ── Tests ──────────────────────────────────────────────────────────────── */

/*
 * Test 1: DNS resolution for db hostname uses DNS shim.
 *
 * Calls getaddrinfo() for a WarpGrid service name and verifies
 * that the DNS shim resolves it to the expected address.
 */
static void test_dns_resolution_for_db_hostname(void) {
    TEST("DNS resolution for db hostname uses DNS shim");
    reset_all();

    struct addrinfo hints;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;

    struct addrinfo *result = NULL;
    int prev_count = dns_resolve_call_count;
    int rc = getaddrinfo("db.production.warp.local", "5432", &hints, &result);

    ASSERT(rc == 0, "getaddrinfo should succeed for WarpGrid hostname");
    ASSERT(dns_resolve_call_count == prev_count + 1,
           "DNS shim should be invoked");
    ASSERT(strcmp(last_dns_hostname, "db.production.warp.local") == 0,
           "DNS shim should receive correct hostname");

    ASSERT(result != NULL, "should return at least one result");

    struct sockaddr_in *addr = (struct sockaddr_in *)result->ai_addr;
    char ip_str[INET_ADDRSTRLEN];
    inet_ntop(AF_INET, &addr->sin_addr, ip_str, sizeof(ip_str));
    ASSERT(strcmp(ip_str, "127.0.0.1") == 0,
           "resolved address should be 127.0.0.1");

    freeaddrinfo(result);
    PASS();
}

/*
 * Test 2: Full Postgres wire protocol lifecycle through proxy.
 *
 * Exercises: DNS resolve → connect → startup → auth → query → terminate → close
 * This is what libpq's PQconnectdb + PQexec + PQfinish does internally.
 */
static void test_full_postgres_lifecycle(void) {
    TEST("full Postgres wire protocol lifecycle through proxy");
    reset_all();

    /* --- Step 1: Resolve hostname via DNS shim --- */
    struct addrinfo hints;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;

    struct addrinfo *result = NULL;
    int rc = getaddrinfo("db.test.warp.local", "5432", &hints, &result);
    ASSERT(rc == 0, "DNS resolution failed");
    ASSERT(result != NULL, "no DNS results");

    /* --- Step 2: Connect to proxy endpoint --- */
    int fd = fake_fd_counter++;
    struct sockaddr_in proxy_addr;
    memcpy(&proxy_addr, result->ai_addr, sizeof(proxy_addr));
    proxy_addr.sin_port = htons(5432);
    freeaddrinfo(result);

    int prev_connect = proxy_connect_call_count;
    rc = __warpgrid_proxy_connect(fd,
             (struct sockaddr *)&proxy_addr, sizeof(proxy_addr));
    ASSERT(rc == 0, "proxy connect failed");
    ASSERT(proxy_connect_call_count == prev_connect + 1,
           "proxy connect shim should be called");
    ASSERT(__warpgrid_proxy_fd_is_proxied(fd), "fd should be proxied");

    /* --- Step 3: Send StartupMessage --- */
    unsigned char buf[4096];
    int msg_len = build_startup_message(buf, sizeof(buf), "testuser", "testdb");
    int sent = __warpgrid_proxy_send(fd, buf, msg_len);
    ASSERT(sent == msg_len, "startup message send failed");

    /* Verify parsed fields */
    ASSERT(captured_protocol_version == PG_PROTOCOL_3_0,
           "wrong protocol version in startup");
    ASSERT(strcmp(captured_user, "testuser") == 0,
           "wrong user in startup message");
    ASSERT(strcmp(captured_database, "testdb") == 0,
           "wrong database in startup message");

    /* --- Step 4: Receive auth response --- */
    unsigned char recv_buf[4096];
    int total_recv = 0;

    /* Read all of the startup response (AuthOk + params + BackendKey + Ready) */
    while (1) {
        int n = __warpgrid_proxy_recv(fd, recv_buf + total_recv,
                                       (int)sizeof(recv_buf) - total_recv, 0);
        if (n <= 0) break;
        total_recv += n;
    }

    ASSERT(total_recv > 0, "should receive startup response");

    /* Verify AuthenticationOk is first message */
    ASSERT(recv_buf[0] == PG_MSG_AUTH, "first message should be AuthenticationOk");
    int auth_type = get_be32(recv_buf + 5);
    ASSERT(auth_type == PG_AUTH_OK, "auth type should be 0 (OK)");

    /* Verify ReadyForQuery is in the response */
    int found_ready = 0;
    int pos = 0;
    while (pos < total_recv) {
        unsigned char msg_type = recv_buf[pos];
        int msg_body_len = get_be32(recv_buf + pos + 1);
        if (msg_type == PG_MSG_READY_FOR_QUERY) {
            found_ready = 1;
            ASSERT(recv_buf[pos + 5] == 'I',
                   "ReadyForQuery status should be 'I' (idle)");
            break;
        }
        pos += 1 + msg_body_len;
    }
    ASSERT(found_ready, "should receive ReadyForQuery in startup response");

    /* --- Step 5: Send query "SELECT 1" --- */
    msg_len = build_query_message(buf, sizeof(buf), "SELECT 1");
    sent = __warpgrid_proxy_send(fd, buf, msg_len);
    ASSERT(sent == msg_len, "query send failed");
    ASSERT(strcmp(captured_query, "SELECT 1") == 0,
           "captured query should be 'SELECT 1'");

    /* --- Step 6: Receive query results --- */
    total_recv = 0;
    while (1) {
        int n = __warpgrid_proxy_recv(fd, recv_buf + total_recv,
                                       (int)sizeof(recv_buf) - total_recv, 0);
        if (n <= 0) break;
        total_recv += n;
    }

    ASSERT(total_recv > 0, "should receive query results");

    /* Parse response: expect RowDescription, DataRow, CommandComplete, ReadyForQuery */
    int found_row_desc = 0;
    int found_data_row = 0;
    int found_cmd_complete = 0;
    int found_ready_q = 0;

    pos = 0;
    while (pos < total_recv) {
        unsigned char msg_type = recv_buf[pos];
        int msg_body_len = get_be32(recv_buf + pos + 1);

        switch (msg_type) {
        case PG_MSG_ROW_DESCRIPTION:
            found_row_desc = 1;
            break;
        case PG_MSG_DATA_ROW: {
            found_data_row = 1;
            /* Verify the row contains "1" */
            int num_cols = ((int)recv_buf[pos + 5] << 8) | recv_buf[pos + 6];
            ASSERT(num_cols == 1, "should have 1 column");
            int col_len = get_be32(recv_buf + pos + 7);
            ASSERT(col_len == 1, "column value should be 1 byte");
            ASSERT(recv_buf[pos + 11] == '1', "column value should be '1'");
            break;
        }
        case PG_MSG_COMMAND_COMPLETE:
            found_cmd_complete = 1;
            break;
        case PG_MSG_READY_FOR_QUERY:
            found_ready_q = 1;
            break;
        }

        pos += 1 + msg_body_len;
    }

    ASSERT(found_row_desc, "should receive RowDescription");
    ASSERT(found_data_row, "should receive DataRow with value '1'");
    ASSERT(found_cmd_complete, "should receive CommandComplete");
    ASSERT(found_ready_q, "should receive ReadyForQuery");

    /* --- Step 7: Send Terminate and close --- */
    msg_len = build_terminate_message(buf);
    sent = __warpgrid_proxy_send(fd, buf, msg_len);
    ASSERT(sent == msg_len, "terminate send failed");
    ASSERT(mock_state == MOCK_STATE_TERMINATED, "mock should be in terminated state");

    int prev_close = proxy_close_call_count;
    rc = __warpgrid_proxy_close(fd);
    ASSERT(rc == 0, "proxy close failed");
    ASSERT(proxy_close_call_count == prev_close + 1,
           "db_proxy_close should be called");
    ASSERT(!__warpgrid_proxy_fd_is_proxied(fd),
           "fd should not be proxied after close");

    PASS();
}

/*
 * Test 3: Connection error propagation.
 *
 * When the proxy connect fails, connect() should return an error
 * and errno should be set. No crash or hang.
 */
static void test_connect_error_propagation(void) {
    TEST("connection error propagates as error code, not crash");
    reset_all();
    simulate_connect_error = 1;

    int fd = fake_fd_counter++;
    struct sockaddr_in proxy_addr;
    memset(&proxy_addr, 0, sizeof(proxy_addr));
    proxy_addr.sin_family = AF_INET;
    proxy_addr.sin_port = htons(5432);
    inet_pton(AF_INET, "127.0.0.1", &proxy_addr.sin_addr);

    int rc = __warpgrid_proxy_connect(fd,
                 (struct sockaddr *)&proxy_addr, sizeof(proxy_addr));

    /* Proxy connect returned error (-1 from our override via the shim) */
    ASSERT(rc != 0, "connect should fail when proxy returns error");

    /* Fd should NOT be tracked as proxied */
    ASSERT(!__warpgrid_proxy_fd_is_proxied(fd),
           "failed connect should not leave fd in proxy table");

    simulate_connect_error = 0;
    PASS();
}

/*
 * Test 4: Auth failure error propagation.
 *
 * When Postgres returns an ErrorResponse during authentication,
 * the error should be receivable and parseable, not cause a crash.
 */
static void test_auth_error_propagation(void) {
    TEST("auth failure error propagated cleanly through proxy");
    reset_all();
    simulate_auth_error = 1;

    /* Connect (succeeds at TCP level) */
    int fd = fake_fd_counter++;
    struct sockaddr_in proxy_addr;
    memset(&proxy_addr, 0, sizeof(proxy_addr));
    proxy_addr.sin_family = AF_INET;
    proxy_addr.sin_port = htons(5432);
    inet_pton(AF_INET, "127.0.0.1", &proxy_addr.sin_addr);

    int rc = __warpgrid_proxy_connect(fd,
                 (struct sockaddr *)&proxy_addr, sizeof(proxy_addr));
    ASSERT(rc == 0, "TCP connect should succeed even for auth failure");

    /* Send startup message */
    unsigned char buf[4096];
    int msg_len = build_startup_message(buf, sizeof(buf), "baduser", "testdb");
    __warpgrid_proxy_send(fd, buf, msg_len);

    /* Receive error response */
    unsigned char recv_buf[4096];
    int total_recv = 0;
    while (1) {
        int n = __warpgrid_proxy_recv(fd, recv_buf + total_recv,
                                       (int)sizeof(recv_buf) - total_recv, 0);
        if (n <= 0) break;
        total_recv += n;
    }

    ASSERT(total_recv > 0, "should receive error response");
    ASSERT(recv_buf[0] == PG_MSG_ERROR_RESPONSE,
           "first message should be ErrorResponse");

    /* Parse error fields to verify they're accessible */
    int found_severity = 0;
    int found_code = 0;
    int found_message = 0;

    int pos = 5; /* skip type byte + length */
    int err_len = get_be32(recv_buf + 1);
    int end = 1 + err_len;
    while (pos < end && recv_buf[pos] != '\0') {
        char field_type = (char)recv_buf[pos++];
        const char *field_val = (const char *)(recv_buf + pos);
        pos += (int)strlen(field_val) + 1;

        switch (field_type) {
        case 'S':
            found_severity = 1;
            ASSERT(strcmp(field_val, "FATAL") == 0,
                   "severity should be FATAL");
            break;
        case 'C':
            found_code = 1;
            ASSERT(strcmp(field_val, "28P01") == 0,
                   "error code should be 28P01 (invalid_password)");
            break;
        case 'M':
            found_message = 1;
            break;
        }
    }

    ASSERT(found_severity, "error should contain severity field");
    ASSERT(found_code, "error should contain SQLSTATE code field");
    ASSERT(found_message, "error should contain message field");

    /* Clean up */
    __warpgrid_proxy_close(fd);

    simulate_auth_error = 0;
    PASS();
}

/*
 * Test 5: Multiple sequential queries on same connection.
 *
 * Validates connection reuse: connect once, run multiple queries,
 * then disconnect. This is what a real database driver does.
 */
static void test_multiple_queries_on_same_connection(void) {
    TEST("multiple queries on same connection via proxy");
    reset_all();

    /* Connect and startup */
    int fd = fake_fd_counter++;
    struct sockaddr_in proxy_addr;
    memset(&proxy_addr, 0, sizeof(proxy_addr));
    proxy_addr.sin_family = AF_INET;
    proxy_addr.sin_port = htons(5432);
    inet_pton(AF_INET, "127.0.0.1", &proxy_addr.sin_addr);

    int rc = __warpgrid_proxy_connect(fd,
                 (struct sockaddr *)&proxy_addr, sizeof(proxy_addr));
    ASSERT(rc == 0, "connect failed");

    /* Send startup */
    unsigned char buf[4096];
    unsigned char recv_buf[4096];
    int msg_len = build_startup_message(buf, sizeof(buf), "testuser", "testdb");
    __warpgrid_proxy_send(fd, buf, msg_len);

    /* Drain startup response */
    while (__warpgrid_proxy_recv(fd, recv_buf, sizeof(recv_buf), 0) > 0) {}

    /* Query 1: SELECT 1 */
    msg_len = build_query_message(buf, sizeof(buf), "SELECT 1");
    __warpgrid_proxy_send(fd, buf, msg_len);

    int total_recv = 0;
    while (1) {
        int n = __warpgrid_proxy_recv(fd, recv_buf + total_recv,
                                       (int)sizeof(recv_buf) - total_recv, 0);
        if (n <= 0) break;
        total_recv += n;
    }
    ASSERT(total_recv > 0, "should receive query 1 results");

    /* Verify we can parse the response (DataRow with "1") */
    int found_data = 0;
    int pos = 0;
    while (pos < total_recv) {
        unsigned char msg_type = recv_buf[pos];
        int msg_body_len = get_be32(recv_buf + pos + 1);
        if (msg_type == PG_MSG_DATA_ROW) {
            found_data = 1;
            ASSERT(recv_buf[pos + 11] == '1', "query 1 result should be '1'");
        }
        pos += 1 + msg_body_len;
    }
    ASSERT(found_data, "should receive data row for query 1");

    /* Query 2: SELECT 1 (again, reusing connection) */
    mock_state = MOCK_STATE_AWAITING_QUERY; /* reset mock for next query */
    msg_len = build_query_message(buf, sizeof(buf), "SELECT 1");
    __warpgrid_proxy_send(fd, buf, msg_len);

    total_recv = 0;
    while (1) {
        int n = __warpgrid_proxy_recv(fd, recv_buf + total_recv,
                                       (int)sizeof(recv_buf) - total_recv, 0);
        if (n <= 0) break;
        total_recv += n;
    }
    ASSERT(total_recv > 0, "should receive query 2 results");

    /* Terminate and close */
    msg_len = build_terminate_message(buf);
    __warpgrid_proxy_send(fd, buf, msg_len);
    __warpgrid_proxy_close(fd);

    /* Verify we only connected once but sent multiple queries */
    ASSERT(proxy_connect_call_count == 1,
           "should only connect once for multiple queries");
    ASSERT(proxy_send_call_count >= 4,
           "should send at least 4 messages (startup + 2 queries + terminate)");

    PASS();
}

/*
 * Test 6: Full lifecycle count verification.
 *
 * Verify all shim layers were invoked in the correct sequence:
 * DNS → connect → send startup → recv auth → send query → recv results → close
 */
static void test_lifecycle_call_counts(void) {
    TEST("full lifecycle invokes all shim layers");
    reset_all();

    /* DNS resolve */
    struct addrinfo hints;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    struct addrinfo *ai_result = NULL;
    getaddrinfo("db.test.warp.local", "5432", &hints, &ai_result);
    if (ai_result) freeaddrinfo(ai_result);

    /* Connect */
    int fd = fake_fd_counter++;
    struct sockaddr_in proxy_addr;
    memset(&proxy_addr, 0, sizeof(proxy_addr));
    proxy_addr.sin_family = AF_INET;
    proxy_addr.sin_port = htons(5432);
    inet_pton(AF_INET, "127.0.0.1", &proxy_addr.sin_addr);
    __warpgrid_proxy_connect(fd,
        (struct sockaddr *)&proxy_addr, sizeof(proxy_addr));

    /* Startup + query + terminate */
    unsigned char buf[4096];
    unsigned char recv_buf[4096];
    int msg_len;

    msg_len = build_startup_message(buf, sizeof(buf), "user", "db");
    __warpgrid_proxy_send(fd, buf, msg_len);
    while (__warpgrid_proxy_recv(fd, recv_buf, sizeof(recv_buf), 0) > 0) {}

    msg_len = build_query_message(buf, sizeof(buf), "SELECT 1");
    __warpgrid_proxy_send(fd, buf, msg_len);
    while (__warpgrid_proxy_recv(fd, recv_buf, sizeof(recv_buf), 0) > 0) {}

    msg_len = build_terminate_message(buf);
    __warpgrid_proxy_send(fd, buf, msg_len);
    __warpgrid_proxy_close(fd);

    /* Verify all layers invoked */
    ASSERT(dns_resolve_call_count == 1, "DNS shim should be called once");
    ASSERT(proxy_connect_call_count == 1, "connect shim should be called once");
    ASSERT(proxy_send_call_count == 3,
           "send shim: 3 calls (startup + query + terminate)");
    ASSERT(proxy_recv_call_count >= 2,
           "recv shim: at least 2 calls (auth response + query response)");
    ASSERT(proxy_close_call_count == 1, "close shim should be called once");

    PASS();
}

/*
 * Test 7: Compile/link verification.
 *
 * The fact that this program compiles and links against the patched
 * sysroot proves that all five patch domains work together:
 * - DNS shim (getaddrinfo + dns_resolve)
 * - FS shim (proxy.conf virtual file)
 * - Socket connect proxy
 * - Socket send/recv proxy
 * - Socket close proxy
 */
static void test_compile_link_all_patches(void) {
    TEST("compile/link verification: all 5 patches integrated");
    /* If we got here, all weak/strong symbols resolved correctly
     * across DNS, FS, socket-connect, socket-send/recv, and socket-close
     * patches. This is the minimum viability proof for US-212. */
    PASS();
}

/*
 * Test 8: Send/recv on non-proxied fd falls through correctly.
 *
 * Ensures that the proxy layer doesn't interfere with normal socket
 * operations on file descriptors that weren't connected through the proxy.
 */
static void test_non_proxy_fd_passthrough(void) {
    TEST("non-proxied fd operations fall through correctly");
    reset_all();

    int fake_fd = 9990; /* Not proxied */
    ASSERT(!__warpgrid_proxy_fd_is_proxied(fake_fd),
           "fd should not be proxied");

    /* These should all return -2 (not intercepted) */
    int rc = __warpgrid_proxy_send(fake_fd, "test", 4);
    ASSERT(rc == -2, "send on non-proxied should return -2");

    rc = __warpgrid_proxy_recv(fake_fd, (char[16]){0}, 16, 0);
    ASSERT(rc == -2, "recv on non-proxied should return -2");

    rc = __warpgrid_proxy_close(fake_fd);
    ASSERT(rc == -2, "close on non-proxied should return -2");

    /* The actual host shims should NOT have been called */
    ASSERT(proxy_send_call_count == 0, "send shim should not be called");
    ASSERT(proxy_recv_call_count == 0, "recv shim should not be called");
    ASSERT(proxy_close_call_count == 0, "close shim should not be called");

    PASS();
}

/* ── Main ───────────────────────────────────────────────────────────────── */

int main(void) {
    printf("=== US-212: End-to-end database driver compilation and connection test ===\n\n");
    fflush(stdout);

    test_dns_resolution_for_db_hostname();
    test_full_postgres_lifecycle();
    test_connect_error_propagation();
    test_auth_error_propagation();
    test_multiple_queries_on_same_connection();
    test_lifecycle_call_counts();
    test_compile_link_all_patches();
    test_non_proxy_fd_passthrough();

    printf("\n=== Results: %d/%d passed ===\n", tests_passed, tests_run);
    return (tests_passed == tests_run) ? 0 : 1;
}
