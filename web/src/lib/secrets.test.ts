import { describe, expect, it } from "vitest";
import { isSecretKey, splitEnv } from "./secrets";

describe("isSecretKey", () => {
  it("matches password, secret, token and key names in any case", () => {
    for (const k of ["POSTGRES_PASSWORD", "db_pass", "AWS_SECRET_ACCESS_KEY", "GITHUB_TOKEN", "apiKey"]) {
      expect(isSecretKey(k)).toBe(true);
    }
    for (const k of ["PATH", "HOME", "NGINX_VERSION"]) expect(isSecretKey(k)).toBe(false);
  });
});

describe("splitEnv", () => {
  it("splits at the first equals sign", () => {
    expect(splitEnv("A=b=c")).toEqual(["A", "b=c"]);
    expect(splitEnv("EMPTY=")).toEqual(["EMPTY", ""]);
    expect(splitEnv("BARE")).toEqual(["BARE", ""]);
  });
});
