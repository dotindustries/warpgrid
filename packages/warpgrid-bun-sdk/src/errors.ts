/**
 * WarpGrid SDK error types.
 *
 * All SDK-specific errors extend from base WarpGridError.
 * Database errors carry the original cause for debugging.
 */

/** Base error for all WarpGrid SDK errors. */
export class WarpGridError extends Error {
  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "WarpGridError";
  }
}

/**
 * Thrown when a database operation fails — connection, query, or pool lifecycle.
 * Always carries the original error as `cause` when available.
 */
export class WarpGridDatabaseError extends WarpGridError {
  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "WarpGridDatabaseError";
  }
}

/** Thrown when DNS resolution fails. */
export class WarpGridDnsError extends WarpGridError {
  readonly code: string;

  constructor(message: string, code: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "WarpGridDnsError";
    this.code = code;
  }
}

/** Thrown when handler validation fails (missing or invalid `fetch` method). */
export class WarpGridHandlerValidationError extends WarpGridError {
  constructor(message: string) {
    super(message);
    this.name = "WarpGridHandlerValidationError";
  }
}

/**
 * Postgres-specific error carrying severity, SQLSTATE code, message, and detail.
 * Surfaces structured error information from the Postgres wire protocol.
 */
export class PostgresError extends WarpGridDatabaseError {
  readonly severity: string;
  readonly code: string;
  readonly detail?: string;

  constructor(fields: {
    severity: string;
    code: string;
    message: string;
    detail?: string;
  }) {
    super(fields.message);
    this.name = "PostgresError";
    this.severity = fields.severity;
    this.code = fields.code;
    this.detail = fields.detail;
  }
}
