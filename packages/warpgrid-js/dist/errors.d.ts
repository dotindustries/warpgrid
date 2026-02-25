/**
 * Base error class for all WarpGrid SDK errors.
 *
 * Thrown when WIT binding calls fail or configuration is invalid.
 * Preserves the original cause for debugging.
 */
export declare class WarpGridError extends Error {
    readonly name = "WarpGridError";
    constructor(message: string, options?: ErrorOptions);
}
//# sourceMappingURL=errors.d.ts.map