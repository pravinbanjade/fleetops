import { describe, expect, it } from "vitest";
import { container } from "@/test/docker";
import { containerQuick, type ContainerQuick } from "./quick";

const names = (q: ContainerQuick[]) =>
  q.map((x) => (x.type === "open" ? x.tab : x.type === "action" ? `${x.action}:${x.rows.length}` : `remove:${x.rows.length}`));
const up = (id: string) => container({ id, name: id, state: "running" });
const down = (id: string) => container({ id, name: id, state: "exited" });

describe("containerQuick", () => {
  it("offers a running container's logs, shell and stats, then what fits its state", () => {
    expect(names(containerQuick([up("a")], true))).toEqual(["logs", "terminal", "stats", "stop:1", "restart:1", "kill:1", "pause:1", "remove:1"]);
  });

  it("offers a stopped container its logs, Start and Remove", () => {
    expect(names(containerQuick([down("a")], true))).toEqual(["logs", "start:1", "remove:1"]);
  });

  it("gives a viewer only what they can read", () => {
    expect(names(containerQuick([up("a")], false))).toEqual(["logs", "stats"]);
  });

  it("counts each action against the selected containers it fits", () => {
    expect(names(containerQuick([up("a"), down("b"), up("c")], true))).toEqual([
      "start:1",
      "stop:2",
      "restart:2",
      "kill:2",
      "pause:2",
      "remove:3",
    ]);
  });
});
