import { describe, expect, it } from "vitest";
import { parseRef } from "./images";

describe("parseRef", () => {
  it("splits the tag off, defaulting to latest", () => {
    expect(parseRef("app:1.2")).toEqual({ repo: "app", tag: "1.2" });
    expect(parseRef(" app ")).toEqual({ repo: "app", tag: "latest" });
    expect(parseRef("ghcr.io/org/app:2")).toEqual({ repo: "ghcr.io/org/app", tag: "2" });
  });

  it("does not take a registry port for a tag", () => {
    expect(parseRef("registry:5000/app")).toEqual({ repo: "registry:5000/app", tag: "latest" });
    expect(parseRef("registry:5000/app:3")).toEqual({ repo: "registry:5000/app", tag: "3" });
  });
});
