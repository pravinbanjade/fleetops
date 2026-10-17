import { describe, expect, it } from "vitest";
import { actPerm, kubePerm, rowOf } from "./perms";

describe("kube permissions", () => {
  it("maps kinds to the server's rows", () => {
    expect(rowOf("deployments")).toBe("workloads");
    expect(rowOf("horizontalpodautoscalers")).toBe("workloads");
    expect(rowOf("ingresses")).toBe("networking");
    expect(rowOf("storageclasses")).toBe("storage");
    expect(rowOf("nodes")).toBe("k8s-other");
    expect(kubePerm("pods", "shell")).toBe("pods.shell");
  });
  it("maps actions to operate or delete", () => {
    expect(actPerm("deployments", "scale")).toBe("workloads.operate");
    expect(actPerm("nodes", "cordon")).toBe("k8s-other.operate");
    expect(actPerm("secrets", "delete")).toBe("secrets.delete");
  });
});
