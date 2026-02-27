/**
 * Bun JSON API Handler Logic — pure testable functions.
 *
 * US-605: Realistic Bun handler with JSON parsing, validation, and transformation.
 *
 * Routes:
 *   POST /transform  — parse JSON, validate, transform (uppercase, slug, domain extract)
 *   POST /validate   — validate a payload against a named schema
 *   GET  /health     — health check
 *
 * This module is pure functions with no side effects, making it testable
 * via `bun test` without the ComponentizeJS fetch-event runtime.
 */

// ── Types ────────────────────────────────────────────────────────────

export interface TransformRequest {
  readonly name: string;
  readonly email: string;
  readonly tags?: readonly string[];
}

export interface TransformResult {
  readonly displayName: string;
  readonly emailDomain: string;
  readonly tagCount: number;
  readonly slug: string;
}

export interface ValidationRequest {
  readonly schema: string;
  readonly payload: Record<string, unknown>;
}

export interface ValidationResult {
  readonly valid: boolean;
  readonly errors: readonly string[];
}

export interface ApiResponse {
  readonly success: boolean;
  readonly data?: Record<string, unknown>;
  readonly error?: string;
}

export interface ResponseDescriptor {
  readonly status: number;
  readonly headers: Record<string, string>;
  readonly body: string;
}

export interface RequestDescriptor {
  readonly method: string;
  readonly url: string;
  readonly body: string | null;
}

// ── Response Helpers ─────────────────────────────────────────────────

function jsonResponse(
  status: number,
  data: ApiResponse
): ResponseDescriptor {
  return {
    status,
    headers: { "content-type": "application/json" },
    body: JSON.stringify(data),
  };
}

function successResponse(
  status: number,
  data: Record<string, unknown>
): ResponseDescriptor {
  return jsonResponse(status, { success: true, data });
}

function errorResponse(
  status: number,
  error: string
): ResponseDescriptor {
  return jsonResponse(status, { success: false, error });
}

// ── Transformation Logic ─────────────────────────────────────────────

function slugify(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9\s-]/g, "")
    .replace(/\s+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");
}

function extractDomain(email: string): string {
  const atIndex = email.indexOf("@");
  return atIndex >= 0 ? email.slice(atIndex + 1) : "";
}

function isValidEmail(email: string): boolean {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email);
}

// ── Route Handlers ───────────────────────────────────────────────────

export function handleTransform(body: string | null): ResponseDescriptor {
  if (!body) {
    return errorResponse(400, "Invalid JSON: request body is empty");
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    return errorResponse(400, "Invalid JSON: failed to parse request body");
  }

  const obj = parsed as Record<string, unknown>;
  const errors: string[] = [];

  if (!obj.name || typeof obj.name !== "string") {
    errors.push("name is required and must be a string");
  }
  if (!obj.email || typeof obj.email !== "string") {
    errors.push("email is required and must be a string");
  } else if (!isValidEmail(obj.email as string)) {
    errors.push("email must be a valid email address");
  }

  if (errors.length > 0) {
    return errorResponse(400, errors.join("; "));
  }

  const name = obj.name as string;
  const email = obj.email as string;
  const tags = Array.isArray(obj.tags) ? (obj.tags as string[]) : [];

  const result: TransformResult = {
    displayName: name.trim().toUpperCase(),
    emailDomain: extractDomain(email),
    tagCount: tags.length,
    slug: slugify(name),
  };

  return successResponse(200, result as unknown as Record<string, unknown>);
}

export function handleValidate(body: string | null): ResponseDescriptor {
  if (!body) {
    return errorResponse(400, "Invalid JSON: request body is empty");
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    return errorResponse(400, "Invalid JSON: failed to parse request body");
  }

  const obj = parsed as Record<string, unknown>;
  const schema = obj.schema as string | undefined;
  const payload = obj.payload as Record<string, unknown> | undefined;

  if (!schema || typeof schema !== "string") {
    return errorResponse(400, "schema is required");
  }

  if (!payload || typeof payload !== "object") {
    return errorResponse(400, "payload is required");
  }

  if (schema === "user") {
    const errors = validateUserSchema(payload);
    const result: ValidationResult = {
      valid: errors.length === 0,
      errors,
    };
    return successResponse(200, result as unknown as Record<string, unknown>);
  }

  return errorResponse(400, `Unknown schema: ${schema}`);
}

function validateUserSchema(payload: Record<string, unknown>): string[] {
  const errors: string[] = [];

  if (!payload.name || typeof payload.name !== "string") {
    errors.push("name is required");
  }

  if (!payload.email || typeof payload.email !== "string") {
    errors.push("email is required");
  } else if (!isValidEmail(payload.email as string)) {
    errors.push("email must be a valid email address");
  }

  if (payload.age !== undefined) {
    if (typeof payload.age !== "number" || payload.age <= 0) {
      errors.push("age must be a positive number");
    }
  }

  return errors;
}

export function handleHealth(): ResponseDescriptor {
  return successResponse(200, { status: "ok" });
}

// ── Main Router ──────────────────────────────────────────────────────

export function handleRequest(req: RequestDescriptor): ResponseDescriptor {
  const url = new URL(req.url);
  const { method } = req;

  if (url.pathname === "/transform") {
    if (method !== "POST") {
      return errorResponse(405, "Method Not Allowed: use POST");
    }
    return handleTransform(req.body);
  }

  if (url.pathname === "/validate") {
    if (method !== "POST") {
      return errorResponse(405, "Method Not Allowed: use POST");
    }
    return handleValidate(req.body);
  }

  if (url.pathname === "/health") {
    return handleHealth();
  }

  return errorResponse(404, "Not Found");
}
