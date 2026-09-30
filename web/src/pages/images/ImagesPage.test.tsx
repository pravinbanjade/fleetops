import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { image } from "@/test/docker";

const NGINX = image({ id: "sha256:aaa111222333444", repo_tags: ["nginx:1.27", "nginx:latest"], in_use: true, size: 190_000_000 });
const OLD = image({ id: "sha256:bbb111222333444", repo_tags: [], created: Math.floor(Date.now() / 1000) - 3 * 86400 });

const routes = { ...loggedIn, "GET /api/v1/envs/local/images": { json: [NGINX, OLD] } };
const rows = (screen: typeof import("@testing-library/react").screen) =>
  within(screen.getByRole("table", { name: "Images" })).getAllByRole("row").slice(1);

describe("images list", () => {
  it("lists tags, short ids, sizes and use, newest first", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/images");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    const [first, second] = rows(screen);
    expect(first).toHaveTextContent("nginx:1.27, nginx:latest");
    expect(within(first).getByText("aaa111222333")).toBeInTheDocument();
    expect(within(first).getByText("190.0 MB")).toBeInTheDocument();
    expect(within(first).getByText("in use")).toBeInTheDocument();
    expect(second).toHaveTextContent("<none>");
    expect(within(second).getByText("3 days ago")).toBeInTheDocument();
  });

  it("removes selected images, forcing when asked", async () => {
    const calls = mockFetch({
      ...routes,
      "DELETE /api/v1/envs/local/images/sha256%3Abbb111222333444": { json: { removed: ["sha256:bbb"] } },
    });
    const { screen } = renderApp("/envs/local/images");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select sha256:bbb111222333444" }));
    await userEvent.click(within(screen.getByRole("group", { name: "Selected" })).getByRole("button", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove bbb111222333?" });
    await userEvent.click(within(dialog).getByLabelText(/Force/));
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    expect(await screen.findByText("Removed 1 image")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "DELETE")?.path).toBe(
      "/api/v1/envs/local/images/sha256%3Abbb111222333444?force=true",
    );
  });

  it("shows why an image cannot be removed", async () => {
    mockFetch({
      ...routes,
      "DELETE /api/v1/envs/local/images/sha256%3Aaaa111222333444": () =>
        json(409, { code: "conflict", message: "image is being used by running container web" }),
    });
    const { screen } = renderApp("/envs/local/images");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select sha256:aaa111222333444" }));
    await userEvent.click(within(screen.getByRole("group", { name: "Selected" })).getByRole("button", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("image is being used by running container web");
  });

  it("prunes unused images when asked for all", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/local/images/prune": { json: { deleted: ["a", "b"], space_reclaimed: 300_000_000 } },
    });
    const { screen } = renderApp("/envs/local/images");
    await userEvent.click(await screen.findByRole("button", { name: "Prune" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByLabelText("Also remove every image no container uses"));
    await userEvent.click(within(dialog).getByRole("button", { name: "Prune" }));
    expect(await screen.findByText("Removed 2 images, reclaimed 300.0 MB")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "POST")?.path).toBe("/api/v1/envs/local/images/prune?all=true");
  });
});
