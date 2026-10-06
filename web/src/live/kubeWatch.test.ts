import { describe, expect, it } from "vitest";
import type { KubeList, KubeRow } from "@/api/gen/types.gen";
import { applyWatchEvent } from "./kubeWatch";

const row = (name: string, status = "Running"): KubeRow => ({ name, namespace: "apps", created: 1, status, health: "ok", columns: {} });

describe("applyWatchEvent", () => {
  const base: KubeList = { rows: [row("a"), row("c")], truncated: true };

  it("replaces everything on a snapshot", () => {
    expect(applyWatchEvent(base, { type: "snapshot", rows: [row("z")] })).toEqual({ rows: [row("z")], truncated: false });
    expect(applyWatchEvent(undefined, { type: "snapshot", rows: [] })).toEqual({ rows: [], truncated: false });
  });

  it("inserts added rows in order and updates modified ones in place", () => {
    const added = applyWatchEvent(base, { type: "added", row: row("b") })!;
    expect(added.rows.map((r) => r.name)).toEqual(["a", "b", "c"]);
    const modified = applyWatchEvent(added, { type: "modified", row: row("b", "Pending") })!;
    expect(modified.rows.find((r) => r.name === "b")!.status).toBe("Pending");
    expect(modified.rows).toHaveLength(3);
  });

  it("removes deleted rows by namespace and name", () => {
    expect(applyWatchEvent(base, { type: "deleted", namespace: "apps", name: "a" })!.rows.map((r) => r.name)).toEqual(["c"]);
    expect(applyWatchEvent(base, { type: "deleted", namespace: "other", name: "a" })!.rows).toHaveLength(2);
  });

  it("leaves the list alone on resync and ignores deltas before a list exists", () => {
    expect(applyWatchEvent(base, { type: "resync" })).toBe(base);
    expect(applyWatchEvent(undefined, { type: "added", row: row("x") })).toBeUndefined();
  });
});
