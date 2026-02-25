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
