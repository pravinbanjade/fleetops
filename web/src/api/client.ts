import { client } from "./gen/client.gen";
import type { ErrorBody } from "./gen/types.gen";

/** An error response from the API, carrying the server's `ErrorBody`. */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly requestId: string | null;

  constructor(status: number, body: Partial<ErrorBody> | undefined) {
    super(body?.message || `request failed with status ${status}`);
    this.name = "ApiError";
    this.status = status;
    this.code = body?.code ?? "internal";
    this.requestId = body?.request_id ?? null;
  }
}

let csrfToken: string | undefined;

/** Stores the session's CSRF token; every mutating operation sends it. */
export function setCsrfToken(token: string | undefined) {
  csrfToken = token;
}

export interface ApiOptions {
  /** Called when a request other than the session probes gets a 401. */
  onUnauthorized?: () => void;
}

let unauthorized: (() => void) | undefined;

/** Reports a lost session found outside the HTTP client (the websocket). */
export function reportUnauthorized() {
  unauthorized?.();
}

// Requests whose 401 is an answer, not a lost session.
const PROBES = ["/api/v1/auth/me", "/api/v1/auth/login", "/api/v1/auth/setup"];

/**
 * Points the generated client at this origin, makes every SDK call throw
 * an `ApiError` on failure, and supplies the CSRF header for the
 * operations whose security lists the `csrf` scheme.
 */
export function configureApi(opts: ApiOptions = {}) {
  unauthorized = opts.onUnauthorized;
  client.setConfig({
    baseUrl: window.location.origin,
    credentials: "same-origin",
    throwOnError: true,
    auth: (scheme) => (scheme.name === "x-csrf-token" ? csrfToken : undefined),
  });
  client.interceptors.error.clear();
  client.interceptors.error.use((error, response, request) => {
    // No response: the request never reached the server (network error).
    if (!response || !request) return error;
    const body = typeof error === "object" && error !== null ? (error as Partial<ErrorBody>) : undefined;
    const path = new URL(request.url).pathname;
    if (response.status === 401 && !PROBES.includes(path)) {
      reportUnauthorized();
    }
    return new ApiError(response.status, body);
  });
}

/**
 * A JSON call outside the generated client (the Enterprise endpoints, which
 * the committed Community document does not list). Sends the CSRF token on
 * mutating requests and throws `ApiError` like the SDK.
 */
export async function apiFetch<T>(
  path: string,
  init: { method?: string; body?: string; contentType?: string } = {},
): Promise<T> {
  const method = init.method ?? "GET";
  const headers = new Headers();
  if (init.contentType) headers.set("content-type", init.contentType);
  if (method !== "GET" && csrfToken) headers.set("x-csrf-token", csrfToken);
  const res = await fetch(
    new Request(new URL(path, window.location.origin), { method, headers, body: init.body, credentials: "same-origin" }),
  );
  const text = await res.text();
  let body: unknown;
  try {
    body = text ? JSON.parse(text) : undefined;
  } catch {
    body = undefined;
  }
  if (!res.ok) {
    if (res.status === 401) reportUnauthorized();
    throw new ApiError(res.status, body as Partial<ErrorBody> | undefined);
  }
  return body as T;
}

/**
 * A raw call for bodies the JSON helpers can't carry (form uploads, file
 * downloads): sends the CSRF token on mutating requests and throws
 * `ApiError` on failure; the caller reads the body.
 */
export async function apiRequest(path: string, init: { method?: string; body?: BodyInit; json?: unknown } = {}): Promise<Response> {
  const method = init.method ?? "GET";
  const headers = new Headers();
  let body = init.body;
  if (init.json !== undefined) {
    headers.set("content-type", "application/json");
    body = JSON.stringify(init.json);
  }
  if (method !== "GET" && csrfToken) headers.set("x-csrf-token", csrfToken);
  const res = await fetch(new Request(new URL(path, window.location.origin), { method, headers, body, credentials: "same-origin" }));
  if (!res.ok) {
    let err: unknown;
    try {
      err = await res.json();
    } catch {
      err = undefined;
    }
    if (res.status === 401 && !PROBES.includes(new URL(res.url || path, window.location.origin).pathname)) reportUnauthorized();
    throw new ApiError(res.status, err as Partial<ErrorBody> | undefined);
  }
  return res;
}
