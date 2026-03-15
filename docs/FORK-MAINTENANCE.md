# Fork Maintenance Guide

This document describes how to maintain the WarpGrid wasi-libc patch series: why the patches exist, how to rebase them onto new upstream releases, how to add new patches, and how to troubleshoot common failures.

## Overview

WarpGrid runs user-supplied WebAssembly modules in a sandboxed environment. These modules are compiled against [wasi-libc](https://github.com/WebAssembly/wasi-libc), the standard C library for WASI targets. Stock wasi-libc does not include networking or advanced filesystem support, so WarpGrid maintains a **patch series** that adds shim functions for:

- **DNS resolution** — `getaddrinfo`, `gethostbyname`, `getnameinfo` routed through the WarpGrid DNS shim
- **Filesystem** — `fopen`/`open` routed through the WarpGrid virtual filesystem, plus timezone virtualization
- **Sockets** — `connect`, `send`, `recv`, `read`, `write`, `close` routed through the WarpGrid database proxy

These patches are kept as numbered `git format-patch` files in `libc-patches/` rather than maintained as a full fork. This keeps the delta small, reviewable, and straightforward to rebase when upstream releases a new version.

### Repository layout

```
libc-patches/
  UPSTREAM_REF           # Pinned upstream tag + commit hash
  0001-dns-*.patch       # Patch series (applied in order)
  0002-fs-*.patch
  ...
  tests/                 # C test programs that validate patch behavior
    test_dns_getaddrinfo.c
    test_fs_virtual_open.c
    ...
scripts/
  rebase-libc.sh         # Apply, export, update, validate patches
  build-libc.sh          # Build stock and/or patched sysroots
  test-libc.sh           # Run test programs against sysroots
```

## Upstream Pin Policy

The file `libc-patches/UPSTREAM_REF` pins the exact upstream commit that the patch series applies to:

```
TAG=wasi-sdk-30
COMMIT=ec0effd769df5f05b647216578bcf5d3b5449725
```

**Rules:**

1. **Always pin to a tagged release** (e.g., `wasi-sdk-30`), never to an arbitrary commit. Tags are stable references that correspond to tested wasi-sdk releases.
2. **Update the pin only when rebasing** — `scripts/rebase-libc.sh --update <new-tag>` updates both `TAG` and `COMMIT` automatically.
3. **Version compatibility** — The patched sysroot must be built with the matching wasi-sdk version. If `TAG=wasi-sdk-30`, use wasi-sdk 30.x to compile against it.
4. **Do not edit UPSTREAM_REF by hand** — The rebase script manages it. Manual edits risk pin/reality divergence.

## Patch Series

The current series has 8 patches, applied in order. Later patches may depend on earlier ones.

| Patch | Purpose | Depends on |
|-------|---------|------------|
| `0001-dns-route-getaddrinfo-through-WarpGrid-DNS-shim` | Routes `getaddrinfo()` through the WarpGrid DNS shim | — |
| `0002-fs-route-fopen-open-through-WarpGrid-virtual-filesys` | Routes `fopen()`/`open()` through the WarpGrid virtual filesystem | — |
| `0003-socket-route-connect-through-WarpGrid-database-proxy` | Routes `connect()` through the WarpGrid database proxy | 0001 |
| `0004-socket-route-send-recv-read-write-through-WarpGrid-d` | Routes `send()`/`recv()`/`read()`/`write()` through the proxy | 0003 |
| `0005-warpgrid-patch-close-for-proxied-fd-cleanup-US-211` | Patches `close()` to clean up proxied file descriptors | 0004 |
| `0006-dns-route-gethostbyname-through-WarpGrid-DNS-shim` | Routes `gethostbyname()` through the DNS shim | 0001 |
| `0007-dns-route-getnameinfo-through-WarpGrid-DNS-reverse-r` | Routes `getnameinfo()` through the DNS shim (reverse lookups) | 0001 |
| `0008-fs-timezone-virtual` | Virtualizes timezone data via the WarpGrid filesystem | — |

**Dependency graph:**

```
0001 (DNS getaddrinfo)
├── 0003 (socket connect) → 0004 (socket send/recv) → 0005 (socket close)
├── 0006 (DNS gethostbyname)
└── 0007 (DNS getnameinfo)

0002 (FS fopen/open)        [independent]
0008 (FS timezone)          [independent]
```

## Rebase Workflow

When a new upstream wasi-libc release is available (e.g., `wasi-sdk-31`), follow these steps:

### 1. Validate current patch health

```bash
scripts/rebase-libc.sh --validate
```

This checks patch numbering, ordering, and dependency constraints. Fix any issues before proceeding.

### 2. Attempt the rebase

```bash
scripts/rebase-libc.sh --update wasi-sdk-31
```

This will:
- Clone wasi-libc at the new tag
- Apply each patch in order using `git am --3way`
- Report which patches applied and which conflicted
- On success: update `UPSTREAM_REF`, export rebased patches, move the source checkout into place

### 3. If the rebase succeeds

Build and test to confirm everything works:

```bash
scripts/build-libc.sh --patched
scripts/test-libc.sh --all
```

If all tests pass, commit the updated patches and `UPSTREAM_REF`.

### 4. If the rebase has conflicts

The script reports which patch failed and which files conflict. To resolve:

1. Apply patches up to the conflict manually:
   ```bash
   scripts/rebase-libc.sh --apply
   ```
2. The script stops at the first conflicting patch. Resolve the conflict in the source checkout (`build/src-patched/`).
3. After resolving, commit the fix:
   ```bash
   cd build/src-patched
   git add -A
   git am --continue
   ```
4. Export the updated patch series:
   ```bash
   scripts/rebase-libc.sh --export
   ```
5. Build and test:
   ```bash
   scripts/build-libc.sh --patched
   scripts/test-libc.sh --all
   ```

## Adding a New Patch

To add a new shim or modification to the patched wasi-libc:

1. **Start from a clean applied state:**
   ```bash
   scripts/rebase-libc.sh --apply
   ```

2. **Make your changes** in `build/src-patched/`:
   ```bash
   cd build/src-patched
   # Edit source files...
   ```

3. **Commit with a descriptive message** following the existing convention (`<subsystem>: <description>`):
   ```bash
   git add -A
   git commit -m "dns: route getXXX through WarpGrid DNS shim"
   ```

4. **Export the updated patch series:**
   ```bash
   scripts/rebase-libc.sh --export
   ```
   This regenerates all `libc-patches/*.patch` files with correct numbering.

5. **Add a test program** in `libc-patches/tests/`:
   - Name it `test_<subsystem>_<feature>.c`
   - Include the `WARPGRID_SHIM_REQUIRED` marker comment if the test needs WarpGrid shims (so it gets skipped when testing the stock sysroot)
   - Include the `LIBPQ_REQUIRED` marker if the test depends on libpq

6. **Verify:**
   ```bash
   scripts/build-libc.sh --patched
   scripts/test-libc.sh --all
   scripts/rebase-libc.sh --validate
   ```

7. **Update the dependency table** in this document if the new patch depends on existing patches. Also update the dependency constraints in `rebase-libc.sh`'s `do_validate()` function.

## Troubleshooting

### Context conflict in header changes

**Symptom:** `git am --3way` fails on a patch that modifies files in `libc-bottom-half/headers/` or `libc-top-half/musl/include/`.

**Why it happens:** Upstream modifies the same header files that WarpGrid patches add declarations to. The 3-way merge cannot resolve the context difference automatically.

**Resolution:**
1. Open the conflicting header file in `build/src-patched/`
2. Look for `<<<<<<<` conflict markers
3. Keep both the upstream changes and the WarpGrid additions (WarpGrid typically adds new function declarations or `#include` directives — these rarely conflict semantically)
4. `git add` the resolved file, then `git am --continue`
5. Re-export patches: `scripts/rebase-libc.sh --export`

### Symbol collision

**Symptom:** Build fails with `multiple definition of 'some_function'` or linker errors after a successful rebase.

**Why it happens:** Upstream added a function with the same name as a WarpGrid shim (e.g., upstream implements `getaddrinfo` natively). The patches still add WarpGrid's version, causing a duplicate symbol.

**Resolution:**
1. Check whether upstream's implementation is equivalent to the WarpGrid shim
2. If yes: remove the WarpGrid shim from the patch (the upstream version replaces it). Update the test to remove the `WARPGRID_SHIM_REQUIRED` marker
3. If no: rename or guard the WarpGrid shim with a preprocessor check (e.g., `#ifndef __WASI_LIBC_HAS_GETADDRINFO`)
4. Re-export patches and rebuild

### Build system changes

**Symptom:** Patches apply cleanly but `build-libc.sh` fails because a CMakeLists.txt target or Makefile variable that WarpGrid patches reference has been renamed, moved, or restructured upstream.

**Why it happens:** WarpGrid patches may modify build rules (e.g., adding source files to a target list). If upstream renames or reorganizes those targets, the patch content is syntactically correct but semantically wrong.

**Resolution:**
1. Compare the upstream CMakeLists.txt/Makefile changes with the patch content
2. Update the patch to use the new target names or file paths
3. Re-export and rebuild:
   ```bash
   cd build/src-patched
   # Fix the build files
   git add -A
   git commit --amend
   scripts/rebase-libc.sh --export
   scripts/build-libc.sh --patched
   ```

## Testing

The test harness runs C test programs from `libc-patches/tests/` against built sysroots.

### Modes

```bash
scripts/test-libc.sh                  # Test patched sysroot (default)
scripts/test-libc.sh --stock          # Test stock sysroot only
scripts/test-libc.sh --patched        # Test patched sysroot only
scripts/test-libc.sh --all            # Test both stock and patched sysroots
scripts/test-libc.sh --ci             # Test both + produce JUnit XML report
```

### How `--stock` vs `--patched` works

- Tests with the `WARPGRID_SHIM_REQUIRED` marker are **skipped** when testing the stock sysroot (because the shim functions don't exist in stock wasi-libc)
- Tests with the `LIBPQ_REQUIRED` marker are **skipped** if libpq has not been built
- All other tests run against both sysroots, verifying that WarpGrid patches don't break baseline functionality

### CI mode (`--ci`)

When `--ci` is passed, the harness:
1. Runs both stock and patched test suites (same as `--all`)
2. Generates a JUnit XML report at `test-results/libc-<timestamp>.xml`
3. Exits non-zero if any test fails

The JUnit XML contains `<testsuite>` and `<testcase>` elements with per-test timing, skip reasons, and failure output — compatible with CI systems like GitHub Actions, Jenkins, and GitLab CI.

### Test harness self-tests

The test harness itself is tested by `scripts/test-libc.test.sh`, which validates flag parsing, output format, and JUnit XML generation using mock toolchains (no wasi-sdk or wasmtime required):

```bash
scripts/test-libc.test.sh             # Run all self-tests
scripts/test-libc.test.sh --quick     # Skip tests that require real execution
```
