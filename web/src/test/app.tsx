import { render, screen } from "@testing-library/react";
import { createMemoryHistory } from "@tanstack/react-router";
import { App } from "@/App";
import type { SessionInfo } from "@/api/gen/types.gen";

export const ADMIN: SessionInfo = {
  csrf_token: "csrf-1",
  user: { id: 1, username: "admin", role: "admin", created_at: 0, updated_at: 0 },
};

export const ENVS = [{ id: 1, name: "local", kind: "docker-local", url: "unix:///var/run/docker.sock", created_at: 0, updated_at: 0 }];

/** API routes for a set-up instance with a logged-in admin and one environment. */
export const loggedIn = {
  "GET /api/v1/auth/status": { json: { setup_required: false, setup_open: false } },
  "GET /api/v1/auth/me": { json: ADMIN },
  "GET /api/v1/envs": { json: ENVS },
  "GET /api/v1/envs/local/status": { json: { state: "up", version: { kind: "docker", version: "29.5.2", api_version: "1.52", os: "linux", arch: "arm64" } } },
  "GET /api/v1/envs/local/info": { json: {} },
  "GET /api/v1/envs/local/df": { json: {} },
  "GET /api/v1/envs/local/volumes": { json: [] },
  "GET /api/v1/envs/local/networks": { json: [] },
  "GET /api/v1/envs/local/containers": { json: [] },
  "GET /api/v1/envs/local/events": { json: [] },
};

/** Renders the whole app at `path`; returns the memory history to inspect navigation. */
export function renderApp(path: string) {
  const history = createMemoryHistory({ initialEntries: [path] });
  render(<App history={history} />);
  return { history, screen };
}

/** A kube-config environment, for Kubernetes page tests (with `kubeRoutes`). */
export const KUBE_ENV = {
  id: 2,
  name: "k",
  kind: "kube-config",
  url: "https://k:6443",
  created_at: 0,
  updated_at: 0,
  agent: null,
  kube: { server: "https://k:6443", context: "dev", user_kind: "token" },
};

/** A Kubernetes row, as the API lists it. */
export function kubeRow(name: string, namespace: string | null = null, columns: Record<string, string> = {}, health = "ok") {
  return { name, namespace, created: 1, status: health === "ok" ? "Running" : "2/3", health, columns };
}

/** Routes for the kube environment `k`, on top of `loggedIn`. */
export const kubeRoutes = {
  "GET /api/v1/envs": { json: [...ENVS, KUBE_ENV] },
  "GET /api/v1/envs/k/status": {
    json: { state: "up", version: { kind: "kube", git_version: "v1.32.0", platform: "linux/amd64", nodes: 3, nodes_ready: 3 } },
  },
  "GET /api/v1/envs/k/k8s/namespaces": { json: { rows: [kubeRow("apps"), kubeRow("default")], truncated: false } },
  "GET /api/v1/envs/k/k8s/deployments": {
    json: { rows: [kubeRow("web", "apps", { ready: "2/3", up_to_date: "3", available: "2" }, "warn")], truncated: false },
  },
  "GET /api/v1/envs/k/k8s/pods": {
    json: { rows: [kubeRow("web-1", "apps", { ready: "1/1", restarts: "0", node: "n1", ip: "" })], truncated: false },
  },
  "GET /api/v1/envs/k/k8s/discovery": { json: { kinds: [], failed_groups: [] } },
};

/** A custom resource kind, as discovery lists it. */
export const WIDGETS = {
  api: { group: "example.fleetops.io", version: "v1", resource: "widgets" },
  kind: "Widget",
  namespaced: true,
  verbs: ["get", "list", "watch", "update", "delete"],
  columns: [
    { name: "Size", json_path: ".spec.size", type: "integer" },
    { name: "Expires", json_path: ".status.notAfter", type: "date" },
  ],
  crd: "widgets.example.fleetops.io",
};

/** Discovery and one Widget for `k`, on top of `kubeRoutes`. */
export const genericRoutes = {
  "GET /api/v1/envs/k/k8s/discovery": { json: { kinds: [WIDGETS], failed_groups: ["metrics.k8s.io"] } },
  "GET /api/v1/envs/k/k8s/r/example.fleetops.io/v1/widgets": {
    json: { rows: [{ ...kubeRow("w1", "apps", { Size: "3", Expires: "1790812800" }), status: "Ready" }], truncated: false },
  },
  "GET /api/v1/envs/k/k8s/r/example.fleetops.io/v1/widgets/apps/w1": {
    json: {
      row: { ...kubeRow("w1", "apps", { Size: "3" }), status: "Ready" },
      yaml: "kind: Widget\n",
      conditions: [{ type: "Ready", status: "True", reason: null, message: null }],
      owners: [],
      containers: [],
    },
  },
};
