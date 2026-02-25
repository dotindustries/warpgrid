/**
 * Base error class for all WarpGrid SDK errors.
 *
 * Thrown when WIT binding calls fail or configuration is invalid.
 * Preserves the original cause for debugging.
 */
export class WarpGridError extends Error {
  override readonly name = "WarpGridError";

  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
  }
}
