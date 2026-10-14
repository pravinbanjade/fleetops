import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { kubeRoutes, loggedIn, renderApp } from "@/test/app";
import { mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { podColor, shortPod } from "@/pages/containers/logs/LogView";

const deployment = {
  row: { name: "web", namespace: "apps", created: 1, status: "2/2", health: "ok", columns: { ready: "2/2" } },
  yaml: "kind: Deployment\n",
  conditions: [],
  owners: [],
  containers: ["web", "proxy"],
};
const squares = (restarts: number) =>
  JSON.stringify([{ name: "main", init: false, state: "ready", reason: "", message: "", restarts }]);
const pod = (restarts: number) => ({
  row: { name: "web-1", namespace: "apps", created: 1, status: "Running", health: "ok", columns: { containers: squares(restarts) } },
  yaml: "kind: Pod\n",
  conditions: [],
  owners: [],
  containers: ["main"],
});
const routes = {
  ...loggedIn,
  ...kubeRoutes,
  "GET /api/v1/envs/k/k8s/deployments/apps/web": { json: deployment },
  "GET /api/v1/envs/k/k8s/pods/apps/web-1": { json: pod(0) },
};

async function logsSub(after = 0) {
  await waitFor(() => expect(FakeWebSocket.latest()).toBeTruthy());
  const sock = FakeWebSocket.latest();
  if (sock.readyState !== FakeWebSocket.OPEN) act(() => sock.open());
  let frame: { sub: number; id: string; opts: Record<string, unknown> } | undefined;
  await waitFor(() => {
    frame = sock.frames().filter((f) => f.op === "sub" && f.topic === "logs" && f.sub > after).pop();
    expect(frame).toBeTruthy();
  });
  return { sock, ...frame! };
}

describe("a workload's logs", () => {
  it("subscribes by workload with 100 lines per pod and the template's first container", async () => {
    mockFetch(routes);
    renderApp("/envs/k/k8s/deployments/apps/web?tab=logs");
    const { id, opts } = await logsSub();
    expect(id).toBe("deployments/apps/web");
    expect(opts).toMatchObject({ tail: 100, container: "web", follow: true });
  });

  it("shows each line's pod as a coloured suffix and marker lines dimmed", async () => {
    mockFetch(routes);
    renderApp("/envs/k/k8s/deployments/apps/web?tab=logs");
    const { sock, sub } = await logsSub();
    act(() =>
      sock.receive({
        sub,
        type: "data",
        data: [
          { stream: "stdout", text: "2026-10-14T10:00:01.000000000Z serving", pod: "web-7f9c4-x2lkq" },
          { stream: "meta", text: "▸ web-7f9c4-abcde started", pod: "web-7f9c4-abcde" },
        ],
      }),
    );
    const log = await screen.findByRole("log");
    const suffix = await within(log).findByText("7f9c4-x2lkq");
    expect(suffix).toHaveStyle({ color: podColor("web-7f9c4-x2lkq") });
    expect(within(log).getByText("▸ web-7f9c4-abcde started").closest("[data-index]")).toHaveClass("italic");
  });

  it("switches container from the chips and keeps it in the URL", async () => {
    mockFetch(routes);
    const { history } = renderApp("/envs/k/k8s/deployments/apps/web?tab=logs");
    const first = await logsSub();
    await userEvent.click(await screen.findByRole("radio", { name: /proxy/ }));
    const next = await logsSub(first.sub);
    expect(next.opts).toMatchObject({ container: "proxy" });
    expect(history.location.search).toContain("container=proxy");
  });

  it("reads previous runs without following", async () => {
    mockFetch(routes);
    renderApp("/envs/k/k8s/deployments/apps/web?tab=logs");
    const first = await logsSub();
    await userEvent.click(screen.getByRole("checkbox", { name: /Previous run/ }));
    const next = await logsSub(first.sub);
    expect(next.opts).toMatchObject({ previous: true, follow: false });
    expect(screen.getByRole("checkbox", { name: "Follow" })).not.toBeChecked();
  });
});

describe("a pod's Previous toggle", () => {
  it("is disabled until the container has restarted", async () => {
    mockFetch(routes);
    renderApp("/envs/k/k8s/pods/apps/web-1?tab=logs");
    expect(await screen.findByRole("checkbox", { name: /Previous run/ })).toBeDisabled();
  });

  it("is enabled after a restart", async () => {
    mockFetch({ ...routes, "GET /api/v1/envs/k/k8s/pods/apps/web-1": { json: pod(2) } });
    renderApp("/envs/k/k8s/pods/apps/web-1?tab=logs");
    expect(await screen.findByRole("checkbox", { name: /Previous run/ })).toBeEnabled();
  });
});

describe("pod suffixes", () => {
  it("drop the workload's name, else keep the last five characters", () => {
    expect(shortPod("web-7f9c4-x2lkq", "web-")).toBe("7f9c4-x2lkq");
    expect(shortPod("db-0", "web-")).toBe("db-0".slice(-5));
    expect(podColor("a")).toBe(podColor("a"));
  });
});
