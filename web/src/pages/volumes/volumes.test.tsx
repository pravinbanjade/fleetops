import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { VolumeSummary } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { container } from "@/test/docker";

const vol = (name: string, over: Partial<VolumeSummary> = {}): VolumeSummary => ({
  name,
  driver: "local",
  mountpoint: `/var/lib/docker/volumes/${name}/_data`,
  labels: {},
  in_use: false,
  created_at: "2026-09-01T00:00:00Z",
  ...over,
});

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/volumes": { json: [vol("pgdata", { in_use: true }), vol("scratch")] },
  "GET /api/v1/envs/local/containers": { json: [container({ id: "c1", name: "db", volumes: ["pgdata"] })] },
};
const rows = (screen: typeof import("@testing-library/react").screen) =>
  within(screen.getByRole("table", { name: "Volumes" })).getAllByRole("row").slice(1);

describe("volumes", () => {
  it("lists volumes with the containers using them", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/volumes");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    const [pg, scratch] = rows(screen);
    expect(within(pg).getByRole("link", { name: "db" })).toHaveAttribute("href", "/envs/local/containers/c1");
    expect(within(scratch).getByText("unused")).toBeInTheDocument();
  });

  it("creates a volume with driver options", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/local/volumes": { status: 201, json: vol("nfs1") },
      "GET /api/v1/envs/local/volumes/nfs1": { json: { Name: "nfs1", Driver: "local", Mountpoint: "/x" } },
    });
    const { screen, history } = renderApp("/envs/local/volumes");
    await userEvent.click(await screen.findByRole("button", { name: "Create volume" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "nfs1");
    await userEvent.type(within(dialog).getByLabelText("Driver options"), "type=nfs{enter}o=addr=10.0.0.5");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/volumes/nfs1"));
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      name: "nfs1",
      driver: "local",
      driver_opts: { type: "nfs", o: "addr=10.0.0.5" },
      labels: {},
    });
  });

  it("rejects a malformed option line before sending", async () => {
    const calls = mockFetch(routes);
    const { screen } = renderApp("/envs/local/volumes");
    await userEvent.click(await screen.findByRole("button", { name: "Create volume" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Driver options"), "nonsense");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent('Option "nonsense" must look like key=value');
    expect(calls.some((c) => c.method === "POST")).toBe(false);
  });

  it("removes a volume, showing Docker's refusal for one in use", async () => {
    mockFetch({
      ...routes,
      "DELETE /api/v1/envs/local/volumes/pgdata": () =>
        json(409, { code: "conflict", message: "volume is in use - [c1]" }),
      "DELETE /api/v1/envs/local/volumes/scratch": { status: 204 },
    });
    const { screen } = renderApp("/envs/local/volumes");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select all" }));
    await userEvent.click(within(screen.getByRole("group", { name: "Selected" })).getByRole("button", { name: "Remove" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Remove" }));
    expect(await screen.findByText("Removed 1 of 2 volumes. pgdata: volume is in use - [c1]")).toBeInTheDocument();
  });

  it("prunes unused volumes", async () => {
    mockFetch({ ...routes, "POST /api/v1/envs/local/volumes/prune": { json: { deleted: ["a"], space_reclaimed: 1000 } } });
    const { screen } = renderApp("/envs/local/volumes");
    await userEvent.click(await screen.findByRole("button", { name: "Prune" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Prune" }));
    expect(await screen.findByText("Removed 1 volumes, reclaimed 1.0 KB")).toBeInTheDocument();
  });

  it("shows a volume's details and users", async () => {
    mockFetch({
      ...routes,
      "GET /api/v1/envs/local/volumes/pgdata": {
        json: { Name: "pgdata", Driver: "local", Mountpoint: "/var/lib/docker/volumes/pgdata/_data", Scope: "local", Labels: { app: "shop" } },
      },
    });
    const { screen } = renderApp("/envs/local/volumes/pgdata");
    const d = await screen.findByLabelText("Volume details");
    expect(within(d).getByText("/var/lib/docker/volumes/pgdata/_data")).toBeInTheDocument();
    expect(within(d).getByText("app=shop")).toBeInTheDocument();
    expect(await within(d).findByRole("link", { name: "db" })).toBeInTheDocument();
  });
});
