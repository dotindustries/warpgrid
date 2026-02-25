/**
 * MockNativePool — A Pool implementation that returns canned responses.
 *
 * Used to test the handler in "native mode" with deterministic data,
 * enabling byte-identical parity comparison with the WasmPool backend.
 */

import type { Pool, QueryResult, FieldInfo } from "@warpgrid/bun-sdk/postgres";
import { WarpGridDatabaseError } from "@warpgrid/bun-sdk/errors";

/** Seed users returned by SELECT queries. */
const SEED_USERS = [
  { id: "1", name: "Alice", email: "alice@test.com" },
  { id: "2", name: "Bob", email: "bob@test.com" },
  { id: "3", name: "Charlie", email: "charlie@test.com" },
];

const USER_FIELDS: FieldInfo[] = [
  { name: "id", dataTypeID: 23 },
  { name: "name", dataTypeID: 25 },
  { name: "email", dataTypeID: 25 },
];

/** Auto-incrementing ID for inserted users. */
let nextId = SEED_USERS.length + 1;

/** Track inserted users across queries within the same pool. */
const insertedUsers: Record<string, unknown>[] = [];

export function resetMockState(): void {
  nextId = SEED_USERS.length + 1;
  insertedUsers.length = 0;
}

export class MockNativePool implements Pool {
  private closed = false;
  private shouldFail = false;

  /**
   * Configure the pool to fail on the next query (for error testing).
   */
  setFailMode(fail: boolean): void {
    this.shouldFail = fail;
  }

  async query(sql: string, params?: unknown[]): Promise<QueryResult> {
    if (this.closed) {
      throw new WarpGridDatabaseError("Pool is closed");
    }

    if (this.shouldFail) {
      throw new WarpGridDatabaseError("Database connection failed");
    }

    // Route based on SQL pattern
    if (sql.startsWith("INSERT INTO users")) {
      return this.handleInsert(params);
    }

    if (sql.includes("WHERE id = $1")) {
      return this.handleSelectById(params);
    }

    if (sql.startsWith("SELECT")) {
      return this.handleSelectAll();
    }

    return { rows: [], rowCount: 0, fields: [] };
  }

  async end(): Promise<void> {
    this.closed = true;
  }

  getPoolSize(): number {
    return this.closed ? 0 : 1;
  }

  getIdleCount(): number {
    return this.closed ? 0 : 1;
  }

  private handleInsert(params?: unknown[]): QueryResult {
    const name = String(params?.[0] ?? "");
    const email = String(params?.[1] ?? "");
    const id = String(nextId++);
    const user = { id, name, email };
    insertedUsers.push(user);
    return {
      rows: [user],
      rowCount: 1,
      fields: USER_FIELDS,
    };
  }

  private handleSelectById(params?: unknown[]): QueryResult {
    const id = String(params?.[0] ?? "");
    const allUsers = [...SEED_USERS, ...insertedUsers];
    const user = allUsers.find(
      (u) => String(u.id) === id,
    );

    if (!user) {
      return { rows: [], rowCount: 0, fields: USER_FIELDS };
    }

    return { rows: [user], rowCount: 1, fields: USER_FIELDS };
  }

  private handleSelectAll(): QueryResult {
    const allUsers = [...SEED_USERS, ...insertedUsers];
    return {
      rows: allUsers,
      rowCount: allUsers.length,
      fields: USER_FIELDS,
    };
  }
}
