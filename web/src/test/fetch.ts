import { vi } from "vitest";

export interface Call {
  method: string;
  path: string;
  headers: Headers;
  body: unknown;
}

type Handler = (call: Call) => Response | { status?: number; json?: unknown } | undefined;

/**
 * Stubs `fetch` with `routes`, keyed "METHOD /path". A handler returns a
 * Response or `{status, json}`; unmatched requests get a 404 ErrorBody.
 * Returns the list of calls made, in order.
 */
export function mockFetch(routes: Record<string, Handler | { status?: number; json?: unknown }>) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url);
      const text = await input.text();
      const call: Call = {
        method: input.method,
        path: url.pathname + url.search,
        headers: input.headers,
        body: text ? JSON.parse(text) : undefined,
      };
      calls.push(call);
      const route = routes[`${call.method} ${url.pathname}`];
      const out = typeof route === "function" ? route(call) : route;
      if (out instanceof Response) return out;
      if (!out) return json(404, { code: "not_found", message: `no mock for ${call.method} ${url.pathname}` });
      return out.json === undefined ? new Response(null, { status: out.status ?? 204 }) : json(out.status ?? 200, out.json);
    }),
  );
  return calls;
}

export function json(status: number, body: unknown) {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}
