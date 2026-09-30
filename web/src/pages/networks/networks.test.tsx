import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { NetworkInspect } from "@/api/docker";
import type { NetworkSummary } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { container } from "@/test/docker";

const net = (id: string, name: string, over: Partial<NetworkSummary> = {}): NetworkSummary => ({
  id,
  name,
  driver: "bridge",
  scope: "local",
  internal: false,
  attachable: false,
  ...over,
});

const SHOP: NetworkInspect = {
  Id: "n1",
  Name: "shop",
  Driver: "bridge",
  Scope: "local",
  IPAM: { Config: [{ Subnet: "172.30.0.0/16", Gateway: "172.30.0.1" }] },
  Containers: { c1: { Name: "web", IPv4Address: "172.30.0.2/16", MacAddress: "02:42:ac:1e:00:02" } },
};

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/networks": {
    json: [net("n0", "bridge", { subnet: "172.17.0.0/16" }), net("n1", "shop", { internal: true })],
  },
  "GET /api/v1/envs/local/networks/n1": { json: SHOP },
  "GET /api/v1/envs/local/containers": {
    json: [container({ id: "c1", name: "web" }), container({ id: "c2", name: "worker" })],
  },
};

describe("networks", () => {
  it("lists networks with subnet and flags", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/networks");
    const table = await screen.findByRole("table", { name: "Networks" });
    await waitFor(() => expect(within(table).getAllByRole("row")).toHaveLength(3));
    expect(within(table).getByText("172.17.0.0/16")).toBeInTheDocument();
    expect(within(table).getByText("internal")).toBeInTheDocument();
  });

  it("creates a network with a subnet", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/networks": { status: 201, json: { id: "n1" } } });
    const { screen, history } = renderApp("/envs/local/networks");
    await userEvent.click(await screen.findByRole("button", { name: "Create network" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "shop");
    await userEvent.type(within(dialog).getByLabelText("Subnet"), "172.30.0.0/16");
    await userEvent.type(within(dialog).getByLabelText("Gateway"), "172.30.0.1");
    await userEvent.click(within(dialog).getByLabelText(/Internal/));
    await userEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/networks/n1"));
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      name: "shop",
      driver: "bridge",
      subnet: "172.30.0.0/16",
      gateway: "172.30.0.1",
      internal: true,
      attachable: false,
      labels: {},
    });
  });

  it("asks for a subnet when a gateway is given", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/networks");
    await userEvent.click(await screen.findByRole("button", { name: "Create network" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "x");
    await userEvent.type(within(dialog).getByLabelText("Gateway"), "10.0.0.1");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("A gateway needs a subnet");
  });

  it("shows attached containers and disconnects one", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/networks/n1/disconnect": { status: 204 } });
    const { screen } = renderApp("/envs/local/networks/n1");
    const table = await screen.findByRole("table", { name: "Attached containers" });
    expect(within(table).getByText("172.30.0.2/16")).toBeInTheDocument();
    expect(within(screen.getByLabelText("Network details")).getByText("172.30.0.0/16")).toBeInTheDocument();
    await userEvent.click(within(table).getByRole("button", { name: "Disconnect" }));
    await waitFor(() => expect(calls.find((c) => c.path.endsWith("/disconnect"))?.body).toEqual({ container: "c1" }));
  });

  it("connects a container that is not attached yet", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/networks/n1/connect": { status: 204 } });
    const { screen } = renderApp("/envs/local/networks/n1");
    const select = await screen.findByLabelText("Container to connect");
    await waitFor(() => expect(within(select).getAllByRole("option")).toHaveLength(2));
    expect(within(select).queryByRole("option", { name: "web" })).not.toBeInTheDocument();
    await userEvent.selectOptions(select, "worker");
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(calls.find((c) => c.path.endsWith("/connect"))?.body).toEqual({ container: "c2" }));
  });

  it("shows Docker's error when connecting fails", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/networks/n1/connect": () =>
        json(409, { code: "conflict", message: "container sharing network namespace with another container" }),
    });
    const { screen } = renderApp("/envs/local/networks/n1");
    const select = await screen.findByLabelText("Container to connect");
    await waitFor(() => expect(within(select).getAllByRole("option")).toHaveLength(2));
    await userEvent.selectOptions(select, "worker");
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("sharing network namespace");
  });

  it("offers no remove for Docker's own networks", async () => {
    mockFetch({ ...routes, "GET /api/v1/envs/local/networks/n0": { json: { ...SHOP, Id: "n0", Name: "bridge" } } });
    const { screen } = renderApp("/envs/local/networks/n0");
    expect(await screen.findByRole("heading", { name: "bridge" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove" })).not.toBeInTheDocument();
  });
});
