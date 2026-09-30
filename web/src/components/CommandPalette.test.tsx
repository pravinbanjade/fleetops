import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { ADMIN, ENVS, loggedIn, renderApp } from "@/test/app";
import { container, image, inspect } from "@/test/docker";

const routes = {
  ...loggedIn,
  "GET /api/v1/envs": { json: [...ENVS, { ...ENVS[0], id: 2, name: "staging" }] },
  "GET /api/v1/envs/local/containers": {
    json: [container({ id: "c1", name: "web" }), container({ id: "c2", name: "db", state: "exited" })],
  },
  "GET /api/v1/envs/local/images": { json: [image({ id: "sha256:img1", repo_tags: ["nginx:1.27"] })] },
  "GET /api/v1/envs/local/containers/c1": { json: inspect({ Id: "c1", Name: "/web" }) },
};

async function openPalette(screen: typeof import("@testing-library/react").screen) {
  await screen.findByRole("heading", { level: 1 });
  await userEvent.keyboard("{Control>}k{/Control}");
  return screen.findByRole("dialog", { name: "Command palette" });
}

describe("command palette", () => {
  it("opens with Ctrl-K and jumps to a container", async () => {
    mockFetch(routes);
    const { screen, history } = renderApp("/envs/local");
    const dialog = await openPalette(screen);
    await userEvent.type(within(dialog).getByRole("combobox"), "container web");
    await waitFor(() => expect(within(dialog).getAllByRole("option")[0]).toHaveTextContent("web"));
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/containers/c1"));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("opens from the sidebar button and finds images", async () => {
    mockFetch(routes);
    const { screen, history } = renderApp("/envs/local");
    await userEvent.click(await screen.findByRole("button", { name: /Search/ }));
    const dialog = await screen.findByRole("dialog", { name: "Command palette" });
    await userEvent.click(await within(dialog).findByRole("option", { name: "nginx:1.27" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/images/sha256%3Aimg1"));
  });

  it("runs an action that fits the container's state", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/containers/c2/start": { status: 204 } });
    const { screen } = renderApp("/envs/local");
    const dialog = await openPalette(screen);
    expect(await within(dialog).findByRole("option", { name: "Stop web" })).toBeInTheDocument();
    expect(within(dialog).queryByRole("option", { name: "Stop db" })).not.toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("option", { name: "Start db" }));
    expect(await screen.findByText("Started db")).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST" && c.path.endsWith("/c2/start"))).toBe(true);
  });

  it("reports a failed action", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/containers/c1/stop": () => json(409, { code: "conflict", message: "is already stopped" }),
    });
    const { screen } = renderApp("/envs/local");
    const dialog = await openPalette(screen);
    await userEvent.click(await within(dialog).findByRole("option", { name: "Stop web" }));
    expect(await screen.findByText("Stop web: is already stopped")).toBeInTheDocument();
  });

  it("switches environment and lists admin pages for admins only", async () => {
    mockFetch({ ...routes, "GET /api/v1/envs/staging/status": { json: { state: "down", reason: "x" } } });
    const { screen, history } = renderApp("/envs/local");
    const dialog = await openPalette(screen);
    expect(within(dialog).getByRole("option", { name: "Users" })).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("option", { name: "Switch to staging" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/staging"));
  });

  it("hides admin pages from other users", async () => {
    mockFetch({ ...routes, "GET /api/v1/auth/me": { json: { ...ADMIN, user: { ...ADMIN.user, role: "user" } } } });
    const { screen } = renderApp("/envs/local");
    const dialog = await openPalette(screen);
    expect(within(dialog).getByRole("option", { name: "Containers" })).toBeInTheDocument();
    expect(within(dialog).queryByRole("option", { name: "Users" })).not.toBeInTheDocument();
  });

  it("leaves no palette heading on the page while closed", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local");
    await screen.findByRole("heading", { level: 1 });
    expect(screen.queryByRole("heading", { name: "Command palette" })).not.toBeInTheDocument();
  });
});

describe("shortcuts", () => {
  it("focuses the list filter with /, but not while typing", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/containers");
    const filter = await screen.findByLabelText("Filter containers");
    await userEvent.keyboard("/");
    expect(filter).toHaveFocus();
    await userEvent.keyboard("a/b");
    expect(filter).toHaveValue("a/b");
  });
});
