import { describe, expect, it } from "vitest";
import { accessOf } from "./access";

describe("accessOf", () => {
  it("reads the caller's grant", () => {
    expect(accessOf({ access: { role: "viewer", namespaces: null, grants: [] } })).toMatchObject({ role: "viewer", namespaces: null });
    expect(accessOf({ access: { role: "operator", namespaces: ["a"], grants: [] } })).toMatchObject({ role: "operator", namespaces: ["a"] });
  });
  it("falls back to the built-in role when the server sends no grants", () => {
    const viewer = accessOf({ access: { role: "viewer", namespaces: null } });
    expect(viewer.can("pods.view")).toBe(true);
    expect(viewer.can("containers.reveal")).toBe(true);
    expect(viewer.can("containers.delete")).toBe(false);
    const operator = accessOf({ access: { role: "operator", namespaces: null } });
    expect(operator.can("containers.delete")).toBe(true);
    expect(operator.can("secrets.reveal")).toBe(false);
  });
  it("treats an environment without a grant field as full access", () => {
    expect(accessOf({})).toMatchObject({ role: "operator", namespaces: null });
    expect(accessOf(undefined).can("secrets.reveal")).toBe(true);
  });
});

describe("can", () => {
  const env = {
    access: {
      role: "operator" as const,
      namespaces: null,
      grants: [
        { namespaces: ["a"], permissions: ["pods.delete"] },
        { namespaces: null, permissions: ["pods.view"] },
      ],
    },
  };

  it("applies a permission only where its grant reaches", () => {
    const { can } = accessOf(env);
    expect(can("pods.delete", "a")).toBe(true);
    expect(can("pods.delete", "b")).toBe(false);
    expect(can("pods.delete")).toBe(true);
    expect(can("pods.view", "b")).toBe(true);
    expect(can("secrets.view")).toBe(false);
  });

  it("whole-environment checks need a whole-environment grant", () => {
    const { can } = accessOf(env);
    expect(can("pods.delete", null)).toBe(false);
    expect(can("pods.view", null)).toBe(true);
  });
});
