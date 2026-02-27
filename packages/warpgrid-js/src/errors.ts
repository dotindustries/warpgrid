/**
 * Base error class for all WarpGrid SDK errors.
 *
 * Thrown when WIT binding calls fail or configuration is invalid.
 * Preserves the original cause for debugging.
 */
export class WarpGridError extends Error {
  override readonly name: string = "WarpGridError";

  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
  }
}

/**
 * Error thrown by DNS resolution operations.
 *
 * Includes the hostname that failed to resolve.
 */
export class WarpGridDNSError extends WarpGridError {
  override readonly name = "WarpGridDNSError";
  readonly hostname: string;

  constructor(hostname: string, message: string, options?: ErrorOptions) {
    super(message, options);
    this.hostname = hostname;
  }
}

/**
 * Error thrown by filesystem operations.
 *
 * Includes the path that caused the error.
 */
export class WarpGridFSError extends WarpGridError {
  override readonly name = "WarpGridFSError";
  readonly path: string;

  constructor(path: string, message: string, options?: ErrorOptions) {
    super(message, options);
    this.path = path;
  }
}
