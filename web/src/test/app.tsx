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
  "GET /api/v1/envs/local/status": { json: { state: "up", version: { version: "29.5.2", api_version: "1.52", os: "linux", arch: "arm64" } } },
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
