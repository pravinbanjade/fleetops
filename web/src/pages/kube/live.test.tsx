import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { kubeRoutes, kubeRow, loggedIn, renderApp } from "@/test/app";
import { FakeWebSocket } from "@/test/fake-ws";
import { mockFetch } from "@/test/fetch";

vi.mock("@/components/yaml-editor/CodeMirrorField", () => ({
  default: ({ value, onChange, label }: { value: string; onChange: (v: string) => void; label: string }) => (
    <textarea aria-label={label} value={value} onChange={(e) => onChange(e.target.value)} />
  ),
}));

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
    await vi.waitFor(() => expect(sock.frames().filter((f) => f.topic === "kube-watch" && f.id === "pods/-").length).toBe(2));
  });

  it("subscribes again at once when the user's access changes", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k/k8s/pods");
    const { sock, sub } = await watchSub("pods/-");
    act(() => sock.receive({ sub, type: "end", reason: "access", message: "your access changed" }));
    await vi.waitFor(() => expect(sock.frames().filter((f) => f.topic === "kube-watch" && f.id === "pods/-").length).toBe(2));
    expect(screen.queryByText(/Live updates paused/)).not.toBeInTheDocument();
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

  it("does not let a REST refetch overwrite a live list", async () => {
    const calls = mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/k8s/templates": { json: { deployments: "kind: Deployment\nmetadata:\n  name: x\n  namespace: NAMESPACE\n" } },
      "POST /api/v1/envs/k/k8s/apply": {
        json: [{ index: 0, api_version: "apps/v1", kind: "Deployment", namespace: "apps", name: "x", action: "created", before: null, after: "a: 1\n", error: null }],
      },
    });
    renderApp("/envs/k/k8s/deployments");
    const table = await screen.findByRole("table", { name: "Deployments" });
    const { sock, sub } = await watchSub("deployments/-");
    act(() => sock.receive({ sub, type: "data", data: { type: "snapshot", rows: [kubeRow("live-only", "apps")] } }));
    expect(await within(table).findByRole("link", { name: "live-only" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    await userEvent.click(await screen.findByRole("button", { name: "Review" }));
    await userEvent.click(await screen.findByRole("button", { name: "Apply" }));
    await vi.waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await new Promise((r) => setTimeout(r, 50));
    expect(calls.filter((c) => c.path.startsWith("/api/v1/envs/k/k8s/deployments")).length).toBe(1);
    expect(within(table).getByRole("link", { name: "live-only" })).toBeInTheDocument();
  });

  it("starts a fresh watch when the list changes after a pause", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k/k8s/pods");
    const { sock, sub } = await watchSub("pods/-");
    act(() => sock.receive({ sub, type: "end", reason: "error", message: "pods is forbidden" }));
    await screen.findByText(/Live updates paused/);
    await userEvent.click(within(screen.getByRole("navigation", { name: "Environment" })).getByRole("link", { name: /Deployments/ }));
    await watchSub("deployments/-");
    expect(screen.queryByText(/Live updates paused/)).not.toBeInTheDocument();
  });
});
