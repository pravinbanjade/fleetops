import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { kubeRoutes, kubeRow, loggedIn, renderApp } from "@/test/app";
import { FakeWebSocket } from "@/test/fake-ws";
import { mockFetch } from "@/test/fetch";

async function watchSub(id: string) {
  let frame: { sub: number } | undefined;
  await vi.waitFor(() => {
    const sock = FakeWebSocket.latest();
    if (sock.readyState !== FakeWebSocket.OPEN) act(() => sock.open());
    frame = sock.frames().find((f) => f.topic === "kube-watch" && f.id === id);
    expect(frame).toBeDefined();
  });
  return { sock: FakeWebSocket.latest(), sub: frame!.sub };
}

describe("live Kubernetes pages", () => {
  it("patches a list from its watch without polling", async () => {
    const calls = mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k/k8s/deployments");
    const table = await screen.findByRole("table", { name: "Deployments" });
    const { sock, sub } = await watchSub("deployments/-");
    act(() => sock.receive({ sub, type: "data", data: { type: "added", row: kubeRow("api", "apps", { ready: "1/1" }) } }));
    expect(await within(table).findByRole("link", { name: "api" })).toBeInTheDocument();
    act(() => sock.receive({ sub, type: "data", data: { type: "deleted", namespace: "apps", name: "web" } }));
    await vi.waitFor(() => expect(within(table).queryByRole("link", { name: "web" })).not.toBeInTheDocument());
    expect(calls.filter((c) => c.path.startsWith("/api/v1/envs/k/k8s/deployments")).length).toBe(1);
  });

  it("says when live updates pause, and retries", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k/k8s/pods");
    const { sock, sub } = await watchSub("pods/-");
    act(() => sock.receive({ sub, type: "end", reason: "error", message: "pods is forbidden" }));
    const banner = await screen.findByText(/Live updates paused/);
    expect(banner.closest("[role=status]")).toHaveTextContent("pods is forbidden");
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await vi.waitFor(() => expect(sock.frames().filter((f) => f.topic === "kube-watch").length).toBe(2));
  });

  it("marks an object page when the object is deleted", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/k8s/deployments/apps/web": {
        json: { row: kubeRow("web", "apps"), yaml: "kind: Deployment\n", conditions: [], owners: [], containers: [] },
      },
    });
    renderApp("/envs/k/k8s/deployments/apps/web");
    await screen.findByRole("heading", { name: "web" });
    const { sock, sub } = await watchSub("deployments/apps");
    act(() => sock.receive({ sub, type: "data", data: { type: "deleted", namespace: "apps", name: "web" } }));
    expect(await screen.findByText(/This object was deleted/)).toBeInTheDocument();
  });
});
