import { describe, expect, it } from "vitest";
import type { KubeRow } from "@/api/gen/types.gen";
import { kubeRow } from "@/test/app";
import { kubeQuick, type KubeQuick } from "./quick";

const pod = (name: string, status = "Running") => ({ ...kubeRow(name, "apps"), status }) as KubeRow;
const dep = (name: string) => kubeRow(name, "apps", { ready: "2/2" }) as KubeRow;
const names = (q: KubeQuick[]) => q.map((x) => (x.type === "open" ? x.tab : x.type === "edit" ? "edit" : `${x.act}:${x.rows.length}`));

describe("kubeQuick", () => {
  it("offers a running pod's logs, shell and events before its changes", () => {
    expect(names(kubeQuick("pods", [pod("web-1")], true))).toEqual(["logs", "terminal", "events", "edit", "delete:1"]);
  });

  it("offers no shell into a pod that is not running", () => {
    expect(names(kubeQuick("pods", [pod("job-1", "Succeeded")], true))).toEqual(["logs", "events", "edit", "delete:1"]);
  });

  it("gives a viewer what they can read and nothing they cannot do", () => {
    expect(names(kubeQuick("pods", [pod("web-1")], false))).toEqual(["logs", "events"]);
    expect(names(kubeQuick("deployments", [dep("web")], false))).toEqual(["events"]);
  });

  it("offers only the changes for a selection of several", () => {
    expect(names(kubeQuick("deployments", [dep("web"), dep("api")], true))).toEqual(["restart:2", "scale:2", "suspend:2", "delete:2"]);
    expect(names(kubeQuick("pods", [pod("a"), pod("b")], true))).toEqual(["delete:2"]);
  });

  it("offers a single workload its events, YAML and every change that fits", () => {
    expect(names(kubeQuick("deployments", [dep("web")], true))).toEqual(["events", "edit", "restart:1", "scale:1", "suspend:1", "delete:1"]);
  });

  it("offers nothing on events, which FleetOps only reads", () => {
    expect(kubeQuick("events", [kubeRow("e1", "apps") as KubeRow], true)).toEqual([]);
  });
});
