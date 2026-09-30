import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { container } from "@/test/docker";

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/compose-projects": {
    json: [{ name: "shop", status: "partial", containers: 2, running: 1, services: ["web", "db"] }],
  },
  "GET /api/v1/envs/local/containers": {
    json: [
      container({ id: "c1", name: "shop-web-1", compose_project: "shop" }),
      container({ id: "c2", name: "shop-db-1", compose_project: "shop", state: "exited", status: "Exited (1)" }),
      container({ id: "c3", name: "loner" }),
    ],
  },
};

describe("compose projects", () => {
  it("shows each project's status and containers", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/compose");
    const card = await screen.findByRole("region", { name: "shop" });
    expect(within(card).getByText("partial")).toBeInTheDocument();
    expect(within(card).getByText("1 of 2 running")).toBeInTheDocument();
    expect(within(card).getByText("Services: web, db")).toBeInTheDocument();
    const list = within(card).getByRole("list", { name: "Containers of shop" });
    await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(2));
    expect(within(list).queryByText("loner")).not.toBeInTheDocument();
  });

  it("starts a project and reports containers that failed", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/local/compose-projects/shop/start": {
        json: { affected: 1, errors: ["shop-db-1: port is already allocated"] },
      },
    });
    const { screen } = renderApp("/envs/local/compose");
    const card = await screen.findByRole("region", { name: "shop" });
    await userEvent.click(within(card).getByRole("button", { name: "Start" }));
    expect(
      await screen.findByText("Started 1 container of shop. shop-db-1: port is already allocated"),
    ).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST" && c.path.endsWith("/shop/start"))).toBe(true);
  });

  it("takes a project down after confirming", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/local/compose-projects/shop/down": { json: { affected: 2, errors: [] } },
    });
    const { screen } = renderApp("/envs/local/compose");
    const card = await screen.findByRole("region", { name: "shop" });
    await userEvent.click(within(card).getByRole("button", { name: "Down" }));
    const dialog = await screen.findByRole("dialog", { name: "Take shop down?" });
    expect(calls.some((c) => c.path.endsWith("/down"))).toBe(false);
    await userEvent.click(within(dialog).getByRole("button", { name: "Down" }));
    expect(await screen.findByText("Removed 2 containers of shop")).toBeInTheDocument();
  });

  it("toasts Docker's error when an action fails outright", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/compose-projects/shop/stop": () =>
        json(502, { code: "upstream", message: "Cannot connect to the Docker daemon" }),
    });
    const { screen } = renderApp("/envs/local/compose");
    const card = await screen.findByRole("region", { name: "shop" });
    await userEvent.click(within(card).getByRole("button", { name: "Stop" }));
    expect(await screen.findByText("Cannot connect to the Docker daemon")).toBeInTheDocument();
  });

  it("says when there are none", async () => {
    mockFetch({ ...routes, "GET /api/v1/envs/local/compose-projects": { json: [] } });
    const { screen } = renderApp("/envs/local/compose");
    expect(await screen.findByText("No compose projects in this environment.")).toBeInTheDocument();
  });
});
