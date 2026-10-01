import { describe, expect, it } from "vitest";
import { ENV_DOWN_POLL_MS, envStatusQuery } from "./queries";
import type { EnvStatus } from "./gen/types.gen";

describe("envStatusQuery", () => {
  it("polls the status only while the environment is down", () => {
    const interval = envStatusQuery("local").refetchInterval as (q: { state: { data?: EnvStatus } }) => number | false;
    expect(interval({ state: { data: { state: "down", reason: "gone" } } })).toBe(ENV_DOWN_POLL_MS);
    expect(interval({ state: { data: { state: "up", version: { kind: "docker", version: "29", api_version: "1.52", os: "linux", arch: "arm64" } } } })).toBe(false);
    expect(interval({ state: {} })).toBe(false);
  });
});
