import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { ImageInspect, ImageLayer } from "@/api/docker";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";

const IMG: ImageInspect = {
  Id: "sha256:aaa111222333444",
  RepoTags: ["nginx:1.27"],
  Created: "2026-09-01T00:00:00Z",
  Size: 190_000_000,
  Os: "linux",
  Architecture: "arm64",
  Config: { Cmd: ["nginx", "-g", "daemon off;"], ExposedPorts: { "80/tcp": {} }, Env: ["NGINX_VERSION=1.27.0"] },
};
const LAYERS: ImageLayer[] = [
  { Id: "sha256:aaa", Created: Math.floor(Date.now() / 1000) - 86400, CreatedBy: 'CMD ["nginx"]', Size: 0 },
  { Id: "<missing>", Created: Math.floor(Date.now() / 1000) - 86400, CreatedBy: "RUN apk add nginx", Size: 50_000_000 },
];

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/images/sha256%3Aaaa111222333444": { json: IMG },
  "GET /api/v1/envs/local/images/sha256%3Aaaa111222333444/history": { json: LAYERS },
  "GET /api/v1/envs/local/images": { json: [] },
};
const PATH = "/envs/local/images/sha256%3Aaaa111222333444";

describe("image page", () => {
  it("shows the image's config", async () => {
    mockFetch(routes);
    const { screen } = renderApp(PATH);
    expect(await screen.findByRole("heading", { name: "nginx:1.27" })).toBeInTheDocument();
    const d = screen.getByLabelText("Image details");
    expect(within(d).getByText("linux/arm64")).toBeInTheDocument();
    expect(within(d).getByText('nginx -g "daemon off;"')).toBeInTheDocument();
    expect(within(d).getByText("80/tcp")).toBeInTheDocument();
    expect(within(d).getByText("NGINX_VERSION=1.27.0")).toBeInTheDocument();
  });

  it("lists the layers", async () => {
    mockFetch(routes);
    const { screen } = renderApp(PATH);
    await userEvent.click(await screen.findByRole("tab", { name: "History" }));
    const table = await screen.findByRole("table", { name: "Layers" });
    expect(within(table).getByText("RUN apk add nginx")).toBeInTheDocument();
    expect(within(table).getByText("50.0 MB")).toBeInTheDocument();
  });

  it("tags the image, splitting repo and tag", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/images/sha256%3Aaaa111222333444/tag": { status: 204 } });
    const { screen } = renderApp(PATH);
    await userEvent.click(await screen.findByRole("button", { name: "Tag" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("New tag"), "registry:5000/web:v2");
    await userEvent.click(within(dialog).getByRole("button", { name: "Tag" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.path.endsWith("/tag"))?.body).toEqual({ repo: "registry:5000/web", tag: "v2" });
  });

  it("goes back to the list after removing the image", async () => {
    mockFetch({
      ...routes,
      "DELETE /api/v1/envs/local/images/sha256%3Aaaa111222333444": { json: { removed: ["sha256:aaa"] } },
    });
    const { screen, history } = renderApp(PATH);
    await userEvent.click(await screen.findByRole("button", { name: "Remove" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/images"));
  });

  it("says so when the image is gone", async () => {
    mockFetch({
      ...routes,
      "GET /api/v1/envs/local/images/sha256%3Aaaa111222333444": () => json(404, { code: "not_found", message: "no such image" }),
    });
    const { screen } = renderApp(PATH);
    expect(await screen.findByRole("alert")).toHaveTextContent("This image no longer exists.");
  });

  it("masks secret-looking image env values until clicked", async () => {
    mockFetch({ ...routes, [`GET /api/v1/envs/local/images/sha256%3Aaaa111222333444`]: { json: { ...IMG, Config: { Env: ["API_TOKEN=abc123"] } } } });
    const { screen } = renderApp(PATH);
    const d = await screen.findByLabelText("Image details");
    expect(within(d).getByText(/API_TOKEN=/)).toBeInTheDocument();
    expect(screen.queryByText(/abc123/)).not.toBeInTheDocument();
    await userEvent.click(within(d).getByRole("button", { name: "Show API_TOKEN" }));
    expect(within(d).getByText(/abc123/)).toBeInTheDocument();
  });
});
