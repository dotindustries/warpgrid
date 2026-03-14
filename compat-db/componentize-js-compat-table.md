# ComponentizeJS npm Package Compatibility

**Runtime**: componentize-js 0.18.4
**Shims**: warpgrid-0.1.0
**Tested**: 2026-02-26T00:00:00Z

## Summary

- **Fully compatible**: 14/20
- **Improved by shims**: 6
- **Core APIs available**: 6/11
- **Core APIs improved by shims**: 3

## Compatibility Table

| Package | Version | Import | Function | Realistic | Status | Notes |
|---------|---------|--------|----------|-----------|--------|-------|
| pg | 8.13.1 | pass | pass | pass | pass IMPROVED | IMPROVED: Works via warpgrid/pg database proxy shim wrapp... |
| ioredis | 5.4.2 | fail | fail | fail | fail | Database proxy shim handles byte passthrough but ioredis ... |
| express | 4.21.2 | fail | fail | fail | fail | Express fundamentally requires Node.js http module server... |
| hono | 4.6.15 | pass | pass | pass | pass | Web-standard request/response model, no Node.js dependencies |
| zod | 3.24.1 | pass | pass | pass | pass | Pure TypeScript schema validation, fully compatible |
| jose | 5.9.6 | pass | pass | pass | pass IMPROVED | IMPROVED: PEM key file loading now works via warpgrid.fs.... |
| uuid | 11.0.5 | pass | pass | pass | pass | Uses crypto.getRandomValues() which is available in Starl... |
| lodash-es | 4.17.21 | pass | pass | pass | pass | Pure JavaScript utility library, fully compatible |
| date-fns | 4.1.0 | pass | pass | pass | pass | Pure JavaScript date utilities, no Node.js dependencies |
| nanoid | 5.0.9 | pass | pass | pass | pass | Uses crypto.getRandomValues() which is available in Starl... |
| superjson | 2.2.2 | pass | pass | pass | pass | Pure JavaScript serialization library, fully compatible |
| drizzle-orm | 0.38.4 | pass | pass | pass | pass IMPROVED | IMPROVED: Query execution now works via warpgrid/pg datab... |
| kysely | 0.27.6 | pass | pass | pass | pass IMPROVED | IMPROVED: Query execution now works via warpgrid/pg datab... |
| better-sqlite3 | 11.7.0 | fail | fail | fail | fail | Fundamentally incompatible — native binary addon, not shi... |
| node-fetch | 3.3.2 | pass | fail | fail | fail | Native fetch() is the recommended approach; node-fetch is... |
| axios | 1.7.9 | pass | fail | fail | fail | Native fetch() is the recommended approach; axios adapter... |
| dotenv | 16.4.7 | pass | pass | pass | pass IMPROVED | IMPROVED: .env file reading now works via warpgrid.fs.rea... |
| pino | 9.6.0 | pass | pass | partial | partial IMPROVED | IMPROVED: Basic logging and level configuration work via ... |
| fast-json-stringify | 6.0.1 | pass | pass | pass | pass | Pure JavaScript JSON serialization, fully compatible |
| ajv | 8.17.1 | pass | pass | pass | pass | Pure JavaScript schema validation, fully compatible |

## Core Node.js APIs

| API | Available | Functional Level | Status | Notes |
|-----|-----------|-----------------|--------|-------|
| Buffer | pass | pass | pass | Polyfilled by StarlingMonkey; Buffer.from(), Buffer.alloc... |
| TextEncoder/TextDecoder | pass | pass | pass | Web standard API, natively available in StarlingMonkey |
| crypto | partial | partial | partial | crypto.getRandomValues() and SubtleCrypto available; Node... |
| URL | pass | pass | pass | Web standard API, natively available in StarlingMonkey |
| console | pass | pass | pass | console.log/warn/error available in StarlingMonkey |
| setTimeout | partial | partial | partial | setTimeout exists but WASI request model has no persisten... |
| process.env | pass | pass | pass IMPROVED | IMPROVED: WASI environment variable polyfill provides pro... |
| fs | partial | partial | partial IMPROVED | IMPROVED: Virtual filesystem shim for specific paths; gen... |
| net | fail | fail | fail | WarpGrid bypasses net module via WIT interface for databa... |
| dns | pass | pass | pass IMPROVED | IMPROVED: WarpGrid DNS shim provides service registry + /... |
| http | fail | fail | fail | Node.js http module server/client model incompatible with... |

## Packages Improved by WarpGrid Shims

### pg

- **Before**: fail
- **After**: pass
- **Details**: IMPROVED: Works via warpgrid/pg database proxy shim wrapping warpgrid:shim/database-proxy WIT interface

### jose

- **Before**: partial
- **After**: pass
- **Details**: IMPROVED: PEM key file loading now works via warpgrid.fs.readFile() filesystem shim

### drizzle-orm

- **Before**: fail
- **After**: pass
- **Details**: IMPROVED: Query execution now works via warpgrid/pg database proxy shim as the driver

### kysely

- **Before**: fail
- **After**: pass
- **Details**: IMPROVED: Query execution now works via warpgrid/pg database proxy shim as the dialect driver

### dotenv

- **Before**: fail
- **After**: pass
- **Details**: IMPROVED: .env file reading now works via warpgrid.fs.readFile() filesystem shim

### pino

- **Before**: fail
- **After**: partial
- **Details**: IMPROVED: Basic logging and level configuration work via process.env polyfill; transports still limited


## Core APIs Improved by WarpGrid Shims

### process.env

- **Before**: fail
- **After**: pass
- **Details**: IMPROVED: WASI environment variable polyfill provides process.env access

### fs

- **Before**: fail
- **After**: partial
- **Details**: IMPROVED: Virtual filesystem shim for specific paths; general fs operations still unavailable

### dns

- **Before**: fail
- **After**: pass
- **Details**: IMPROVED: WarpGrid DNS shim provides service registry + /etc/hosts + system DNS resolution

