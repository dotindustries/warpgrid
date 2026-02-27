# ComponentizeJS npm Package Compatibility

**Runtime**: componentize-js 0.18.4
**Shims**: warpgrid-0.1.0
**Tested**: 2026-02-26T00:00:00Z

## Summary

- **Fully compatible**: 14/20 packages
- **Improved by WarpGrid shims**: 6 packages
- **Permanently incompatible**: 5 packages

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

## Packages Improved by WarpGrid Shims

### pg

- **Before** (baseline): fail
- **After** (WarpGrid): pass
- **Details**: IMPROVED: Works via warpgrid/pg database proxy shim wrapping warpgrid:shim/database-proxy WIT interface

### jose

- **Before** (baseline): partial
- **After** (WarpGrid): pass
- **Details**: IMPROVED: PEM key file loading now works via warpgrid.fs.readFile() filesystem shim

### drizzle-orm

- **Before** (baseline): fail
- **After** (WarpGrid): pass
- **Details**: IMPROVED: Query execution now works via warpgrid/pg database proxy shim as the driver

### kysely

- **Before** (baseline): fail
- **After** (WarpGrid): pass
- **Details**: IMPROVED: Query execution now works via warpgrid/pg database proxy shim as the dialect driver

### dotenv

- **Before** (baseline): fail
- **After** (WarpGrid): pass
- **Details**: IMPROVED: .env file reading now works via warpgrid.fs.readFile() filesystem shim

### pino

- **Before** (baseline): fail
- **After** (WarpGrid): partial
- **Details**: IMPROVED: Basic logging and level configuration work via process.env polyfill; transports still limited

