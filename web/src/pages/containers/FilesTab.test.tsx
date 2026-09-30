import { within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { DirListing } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { inspect } from "@/test/docker";

const ROOT: DirListing = {
  path: "/",
  truncated: false,
  entries: [
    { name: "etc", kind: "dir", size: 0, mode: 0o755, modified: 1_790_000_000 },
    { name: "bin", kind: "symlink", size: 0, mode: 0o777, modified: 1_790_000_000, link_target: "usr/bin" },
    { name: "README", kind: "file", size: 1500, mode: 0o644, modified: 1_790_000_000 },
  ],
};
const ETC: DirListing = {
  path: "/etc",
  truncated: true,
  entries: [{ name: "hosts", kind: "file", size: 174, mode: 0o644, modified: 1_790_000_000 }],
};

const listing = (call: { path: string }) => {
  const dir = new URL(call.path, "http://x").searchParams.get("path");
  if (dir === "/") return json(200, ROOT);
  if (dir === "/etc") return json(200, ETC);
  return json(404, { code: "not_found", message: `'${dir}' not found` });
};

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/containers/c1": { json: inspect({ Id: "c1", Name: "/web" }) },
  "GET /api/v1/envs/local/containers/c1/files": listing,
};

describe("files tab", () => {
  it("lists the root with sizes, modes and symlink targets", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/containers/c1?tab=files");
    const table = await screen.findByRole("table", { name: "Files" });
    expect(await within(table).findByText("README")).toBeInTheDocument();
    expect(within(table).getByText("1.5 KB")).toBeInTheDocument();
    expect(within(table).getByText("drwxr-xr-x")).toBeInTheDocument();
    expect(within(table).getByText("→ usr/bin")).toBeInTheDocument();
    expect(within(table).getByRole("link", { name: "Download README" })).toHaveAttribute(
      "href",
      "/api/v1/envs/local/containers/c1/files/download?path=%2FREADME",
    );
  });

  it("opens a directory and goes back through the breadcrumbs", async () => {
    mockFetch(routes);
    const { screen, history } = renderApp("/envs/local/containers/c1?tab=files");
    await userEvent.click(await screen.findByRole("link", { name: "etc" }));
    expect(history.location.search).toContain("path=%2Fetc");
    expect(await screen.findByText("hosts")).toBeInTheDocument();
    expect(screen.getByText(/too large to list in full/)).toBeInTheDocument();
    await userEvent.click(within(screen.getByRole("navigation", { name: "Path" })).getByRole("link", { name: "/" }));
    expect(await screen.findByText("README")).toBeInTheDocument();
  });

  it("shows the server's error for a path that does not exist", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/containers/c1?tab=files&path=%2Fnope");
    expect(await screen.findByRole("alert")).toHaveTextContent("'/nope' not found");
  });
});
