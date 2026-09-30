import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { ContainerSpec } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { inspect } from "@/test/docker";

const SPEC: ContainerSpec = {
  image: "nginx:1.27",
  name: "web",
  cmd: ["nginx", "-g", "daemon off;"],
  env: ["A=1"],
  ports: [{ container_port: 80, host_port: 8080, protocol: "tcp" }],
  binds: [],
  labels: {},
  cap_add: [],
  privileged: false,
  restart_policy: "always",
};

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/networks": { json: [{ id: "n1", name: "shop", driver: "bridge", scope: "local", internal: false, attachable: false }] },
  "GET /api/v1/envs/local/containers/c1/spec": { json: SPEC },
  "GET /api/v1/envs/local/containers/c1": { json: inspect({ Id: "c1", Name: "/web" }) },
  "GET /api/v1/envs/local/containers/new1": { json: inspect({ Id: "new1", Name: "/api" }) },
};

describe("container form", () => {
  it("creates a container from the form and opens it", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/containers": { status: 201, json: { id: "new1" } } });
    const { screen, history } = renderApp("/envs/local/containers/new");
    await userEvent.type(await screen.findByLabelText("Image"), "redis:7");
    await userEvent.type(screen.getByLabelText("Name"), "api");
    await userEvent.type(screen.getByLabelText("Command"), 'redis-server --save ""');
    await userEvent.click(screen.getByRole("button", { name: "Add port" }));
    const port = screen.getByRole("group", { name: "Port 1" });
    await userEvent.type(within(port).getByLabelText("Host port"), "6380");
    await userEvent.type(within(port).getByLabelText("Container port"), "6379");
    await userEvent.type(screen.getByLabelText("Environment variables"), "A=1{enter}B=2");
    await userEvent.selectOptions(await screen.findByLabelText("Network"), "shop");
    await userEvent.selectOptions(screen.getByLabelText("Restart policy"), "unless-stopped");
    await userEvent.type(screen.getByLabelText("Memory limit (MB)"), "256");
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/containers/new1"));
    expect(calls.find((c) => c.method === "POST")?.body).toMatchObject({
      image: "redis:7",
      name: "api",
      cmd: ["redis-server", "--save", ""],
      ports: [{ container_port: 6379, host_port: 6380, protocol: "tcp" }],
      env: ["A=1", "B=2"],
      network: "shop",
      restart_policy: "unless-stopped",
      memory_bytes: 256 * 1024 * 1024,
      start: true,
    });
  });

  it("says which field is wrong without sending anything", async () => {
    const calls = mockFetch(routes);
    const { screen } = renderApp("/envs/local/containers/new");
    await userEvent.type(await screen.findByLabelText("Image"), "redis");
    await userEvent.type(screen.getByLabelText("Labels"), "oops");
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findByRole("alert")).toHaveTextContent('Label "oops" must look like key=value');
    expect(calls.some((c) => c.method === "POST")).toBe(false);
  });

  it("shows Docker's error inline", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/containers": () => json(502, { code: "upstream", message: "No such image: redis:nope" }),
    });
    const { screen } = renderApp("/envs/local/containers/new");
    await userEvent.type(await screen.findByLabelText("Image"), "redis:nope");
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("No such image: redis:nope");
    // Inline only, not also as a toast.
    expect(screen.getAllByText("No such image: redis:nope")).toHaveLength(1);
  });

  it("duplicates a container from its spec under a new name", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/containers": { status: 201, json: { id: "new1" } } });
    const { screen, history } = renderApp("/envs/local/containers/c1");
    await userEvent.click(await screen.findByRole("link", { name: "Duplicate" }));
    expect(await screen.findByRole("heading", { name: "Duplicate web" })).toBeInTheDocument();
    expect(screen.getByLabelText("Name")).toHaveValue("web-copy");
    expect(screen.getByLabelText("Command")).toHaveValue('nginx -g "daemon off;"');
    const port = screen.getByRole("group", { name: "Port 1" });
    await userEvent.clear(within(port).getByLabelText("Host port"));
    await userEvent.type(within(port).getByLabelText("Host port"), "8081");
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/containers/new1"));
    expect(calls.find((c) => c.method === "POST")?.body).toMatchObject({
      name: "web-copy",
      cmd: ["nginx", "-g", "daemon off;"],
      ports: [{ container_port: 80, host_port: 8081, protocol: "tcp" }],
      restart_policy: "always",
    });
  });

  it("recreates a container with changes and opens the replacement", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/local/containers/c1/recreate": {
        json: { id: "new1", old_removed: true, backup_name: "web-fleetops-old-1" },
      },
    });
    const { screen, history } = renderApp("/envs/local/containers/c1");
    await userEvent.click(await screen.findByRole("link", { name: "Edit" }));
    expect(await screen.findByRole("heading", { name: "Edit web" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Start the container after creating it")).not.toBeInTheDocument();
    await userEvent.clear(screen.getByLabelText("Image"));
    await userEvent.type(screen.getByLabelText("Image"), "nginx:1.28");
    await userEvent.click(screen.getByRole("button", { name: "Recreate" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/containers/new1"));
    const body = calls.find((c) => c.path.endsWith("/recreate"))?.body as ContainerSpec;
    expect(body).toMatchObject({ image: "nginx:1.28", name: "web", env: ["A=1"] });
    expect(body).not.toHaveProperty("start");
    expect(await screen.findByText("Container recreated")).toBeInTheDocument();
  });

  it("warns when the old container was kept after a recreate", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/containers/c1/recreate": {
        json: { id: "new1", old_removed: false, backup_name: "web-fleetops-old-1" },
      },
    });
    const { screen } = renderApp("/envs/local/containers/c1/edit");
    await userEvent.click(await screen.findByRole("button", { name: "Recreate" }));
    expect(await screen.findByText(/it is kept as web-fleetops-old-1/)).toBeInTheDocument();
  });
});
