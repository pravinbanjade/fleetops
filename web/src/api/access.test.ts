import { describe, expect, it } from "vitest";
import { accessOf } from "./access";

describe("accessOf", () => {
  it("reads the caller's grant", () => {
    expect(accessOf({ access: { role: "viewer", namespaces: null } })).toEqual({ role: "viewer", canWrite: false, namespaces: null });
    expect(accessOf({ access: { role: "operator", namespaces: ["a"] } })).toEqual({ role: "operator", canWrite: true, namespaces: ["a"] });
  });
  it("treats an environment without a grant field as full access", () => {
    expect(accessOf({})).toEqual({ role: "operator", canWrite: true, namespaces: null });
    expect(accessOf(undefined)).toEqual({ role: "operator", canWrite: true, namespaces: null });
  });
});
