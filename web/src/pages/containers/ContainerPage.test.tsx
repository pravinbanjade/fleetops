import { act, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { loggedIn, renderApp } from "@/test/app";
import { container, inspect } from "@/test/docker";
import { actionsFor } from "./actions";

const WEB = inspect({
  Id: "c1abcdef0123456789",
  Name: "/web",
  Config: {
    Image: "nginx:1.27",
    Cmd: ["nginx", "-g", "daemon off;"],
    Env: ["PATH=/usr/bin", "DB_PASSWORD=hunter2", "GREETING=a=b"],
    Labels: { "com.docker.compose.project": "shop" },
  },
  Mounts: [{ Type: "volume", Name: "data", Source: "/var/lib/docker/volumes/data/_data", Destination: "/data", RW: true }],
  NetworkSettings: {
    Ports: { "80/tcp": [{ HostIp: "0.0.0.0", HostPort: "8080" }] },
    Networks: { bridge: { IPAddress: "172.17.0.2", Gateway: "172.17.0.1", MacAddress: "02:42:ac:11:00:02" } },
  },
});

const routes = { ...loggedIn, "GET /api/v1/envs/local/containers/c1": { json: WEB } };

describe("container page", () => {
  it("shows the overview: state, ports, restart policy and labels", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/containers/c1");
    expect(await screen.findByRole("heading", { name: /web/ })).toHaveTextContent("running");
    const overview = screen.getByLabelText("Container details");
    expect(within(overview).getByText("c1abcdef0123")).toBeInTheDocument();
    expect(within(overview).getByText("0.0.0.0:8080 → 80/tcp")).toBeInTheDocument();
    expect(within(overview).getByText('nginx -g "daemon off;"')).toBeInTheDocument();
    expect(within(overview).getByText("unless-stopped")).toBeInTheDocument();
    expect(within(screen.getByLabelText("Labels")).getByText("shop")).toBeInTheDocument();
  });

  it("opens from the containers list", async () => {
    mockFetch({ ...routes, "GET /api/v1/envs/local/containers": { json: [container({ id: "c1", name: "web" })] } });
    const { screen, history } = renderApp("/envs/local/containers");
    await userEvent.click(await screen.findByRole("link", { name: "web" }));
    expect(history.location.pathname).toBe("/envs/local/containers/c1");
    expect(await screen.findByRole("heading", { name: /web/ })).toBeInTheDocument();
  });

  it("masks secret-looking env values until clicked", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/containers/c1?tab=env");
    const table = await screen.findByRole("table", { name: "Environment" });
    expect(within(table).getByText("/usr/bin")).toBeInTheDocument();
    expect(within(table).getByText("a=b")).toBeInTheDocument();
    expect(within(table).queryByText("hunter2")).not.toBeInTheDocument();
    await userEvent.click(within(table).getByRole("button", { name: "Show DB_PASSWORD" }));
    expect(within(table).getByText("hunter2")).toBeInTheDocument();
  });

  it("switches tabs through the address", async () => {
    mockFetch(routes);
    const { screen, history } = renderApp("/envs/local/containers/c1");
    await screen.findByRole("heading", { name: /web/ });
    await userEvent.click(screen.getByRole("tab", { name: "Mounts" }));
    expect(history.location.search).toBe("?tab=mounts");
    const mounts = screen.getByRole("table", { name: "Mounts" });
    expect(within(mounts).getByText("data")).toBeInTheDocument();
    expect(within(mounts).getByText("read-write")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: "Networks" }));
    expect(within(screen.getByRole("table", { name: "Networks" })).getByText("172.17.0.2")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: "Inspect" }));
    expect(screen.getByLabelText("Container JSON")).toHaveTextContent('"Id": "c1abcdef0123456789"');
  });

  it("offers the actions that fit the state", () => {
    expect(actionsFor("running")).toEqual(["stop", "restart", "kill", "pause"]);
    expect(actionsFor("paused")).toEqual(["unpause", "kill"]);
    expect(actionsFor("exited")).toEqual(["start"]);
    expect(actionsFor("created")).toEqual(["start"]);
  });

  it("stops the container and follows its new state", async () => {
    let state = WEB;
    const calls = mockFetch({
      ...routes,
      "GET /api/v1/envs/local/containers/c1": () => json(200, state),
      "POST /api/v1/envs/local/containers/c1abcdef0123456789/stop": () => {
        state = { ...WEB, State: { Status: "exited", ExitCode: 0 } };
        return { status: 204 };
      },
    });
    const { screen } = renderApp("/envs/local/containers/c1");
    await userEvent.click(await screen.findByRole("button", { name: "Stop" }));
    expect(await screen.findByRole("button", { name: "Start" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /web/ })).toHaveTextContent("exited");
    expect(calls.some((c) => c.method === "POST")).toBe(true);
  });

  it("renames the container, showing Docker's error inline", async () => {
    let tries = 0;
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/local/containers/c1abcdef0123456789/rename": () =>
        tries++ === 0 ? json(409, { code: "conflict", message: "name web2 is already in use" }) : { status: 204 },
    });
    const { screen } = renderApp("/envs/local/containers/c1");
    await userEvent.click(await screen.findByRole("button", { name: "Rename" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.clear(within(dialog).getByLabelText("New name"));
    await userEvent.type(within(dialog).getByLabelText("New name"), "web2");
    await userEvent.click(within(dialog).getByRole("button", { name: "Rename" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("name web2 is already in use");
    await userEvent.click(within(dialog).getByRole("button", { name: "Rename" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.filter((c) => c.path.endsWith("/rename")).at(-1)?.body).toEqual({ name: "web2" });
  });

  it("goes back to the list after removing the container", async () => {
    mockFetch({
      ...routes,
      "DELETE /api/v1/envs/local/containers/c1abcdef0123456789": { status: 204 },
      "GET /api/v1/envs/local/containers": { json: [] },
    });
    const { screen, history } = renderApp("/envs/local/containers/c1");
    await userEvent.click(await screen.findByRole("button", { name: "Remove" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/containers"));
  });

  it("says so when the container is removed elsewhere", async () => {
    let gone = false;
    mockFetch({
      ...routes,
      "GET /api/v1/envs/local/containers/c1": () =>
        gone ? json(404, { code: "not_found", message: "no such container" }) : json(200, WEB),
    });
    const { screen } = renderApp("/envs/local/containers/c1");
    await screen.findByRole("heading", { name: /web/ });
    const sock = FakeWebSocket.latest();
    act(() => sock.open());
    const sub = sock.frames().find((f) => f.topic === "events").sub;
    gone = true;
    act(() =>
      sock.receive({ sub, type: "data", data: { kind: "container", action: "destroy", id: "c1", attributes: {}, time: 1 } }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("This container no longer exists.");
    expect(screen.getByRole("link", { name: "Back to containers" })).toBeInTheDocument();
  });

  it("masks secrets in the Inspect JSON until shown", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/containers/c1?tab=inspect");
    const pre = await screen.findByLabelText("Container JSON");
    expect(pre).toHaveTextContent("DB_PASSWORD=••••••");
    expect(pre).not.toHaveTextContent("hunter2");
    await userEvent.click(screen.getByRole("button", { name: "Show secrets" }));
    expect(screen.getByLabelText("Container JSON")).toHaveTextContent("DB_PASSWORD=hunter2");
  });
});
