import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, configureApi, setCsrfToken } from "./client";
import { getMe, listEnvs, logout } from "./gen/sdk.gen";
import { mockFetch } from "@/test/fetch";

describe("api client", () => {
  beforeEach(() => {
    setCsrfToken(undefined);
    configureApi();
  });

  it("sends the CSRF token on mutations only", async () => {
    const calls = mockFetch({ "GET /api/v1/envs": { json: [] }, "POST /api/v1/auth/logout": { status: 204 } });
    setCsrfToken("tok");
    await listEnvs();
    await logout();
    expect(calls[0].headers.get("x-csrf-token")).toBeNull();
    expect(calls[1].headers.get("x-csrf-token")).toBe("tok");
  });

  it("throws ApiError with the server's error body", async () => {
    mockFetch({
      "GET /api/v1/envs": { status: 503, json: { code: "env_unavailable", message: "docker is down", request_id: "r1" } },
    });
    const err = await listEnvs().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err).toMatchObject({ status: 503, code: "env_unavailable", message: "docker is down", requestId: "r1" });
  });

  it("throws ApiError for a non-JSON error response", async () => {
    mockFetch({ "GET /api/v1/envs": () => new Response("bad gateway", { status: 502 }) });
    const err = await listEnvs().catch((e: unknown) => e);
    expect(err).toMatchObject({ status: 502, code: "internal" });
  });

  it("passes network errors through unchanged", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => Promise.reject(new TypeError("Failed to fetch"))));
    const err = await listEnvs().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(TypeError);
  });

  it("reports a lost session, but not a 401 from the session probe", async () => {
    const onUnauthorized = vi.fn();
    configureApi({ onUnauthorized });
    const lost = { status: 401, json: { code: "unauthorized", message: "no session" } };
    mockFetch({ "GET /api/v1/auth/me": lost, "GET /api/v1/envs": lost });
    await getMe().catch(() => {});
    expect(onUnauthorized).not.toHaveBeenCalled();
    await listEnvs().catch(() => {});
    expect(onUnauthorized).toHaveBeenCalledOnce();
  });
});
