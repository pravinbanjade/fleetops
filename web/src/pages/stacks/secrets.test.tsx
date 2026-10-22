import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { mockFetch } from "@/test/fetch";
import { ENVS, loggedIn, renderApp } from "@/test/app";

const COMPOSE = "services:\n  db:\n    image: postgres\n";

const detail = (source = "inline") => ({
  stack: { id: 1, env_id: 1, name: "shop", source, deployed_revision: null, created_at: 0, created_by: 1 },
  draft: { compose: COMPOSE, variables: [], updated_at: 5 },
  deployed: null,
});

const item = (over: Record<string, unknown>) => ({
  kind: "secret",
  name: "db_password",
  source: "fleetops",
  variable: null,
  path: "./pw.txt",
  used_by: ["db"],
  set: false,
  used: true,
  content: null,
  updated_at: null,
  updated_by: null,
  ...over,
});

const FILES = [
  item({}),
  item({ name: "api_key", set: true, updated_at: 10, updated_by: "admin" }),
  item({ kind: "config", name: "app", path: "./app.conf", set: true, content: "listen 80;" }),
  item({ kind: "config", name: "var_conf", source: "variable", variable: "CONF", path: null, set: true }),
  item({ name: "old", used: false, used_by: [], set: true, path: null }),
];

const plan = (issues: { path: string; message: string }[] = []) => ({
  issues,
  warnings: [],
  plan: issues.length ? null : { services: [], networks: [], volumes: [], images: [], blocking: [], token: "tok" },
  compose_before: null,
  compose_after: COMPOSE,
  variables_changed: [],
});

const routes = (files: unknown[] = FILES, source = "inline") => ({
  ...loggedIn,
  "GET /api/v1/envs/local/stacks/shop": { json: detail(source) },
  "GET /api/v1/envs/local/stacks/shop/files": { json: files },
  "PUT /api/v1/envs/local/stacks/shop/files/secret/db_password": { status: 204 },
  "PUT /api/v1/envs/local/stacks/shop/files/config/app": { status: 204 },
  "DELETE /api/v1/envs/local/stacks/shop/files/secret/old": { status: 204 },
  "POST /api/v1/envs/local/stacks/validate": { json: { issues: [], warnings: [] } },
  "PUT /api/v1/envs/local/stacks/shop/draft": { status: 204 },
  "POST /api/v1/envs/local/stacks/shop/plan": { json: plan() },
  "GET /api/v1/envs/local/stacks/shop/revisions": { json: [] },
});

const withPerms = (permissions: string[]) => ({
  "GET /api/v1/envs": {
    json: ENVS.map((e) => ({ ...e, access: { role: "viewer", namespaces: null, grants: [{ namespaces: null, permissions }] } })),
  },
});

async function openSecrets(screen: ReturnType<typeof renderApp>["screen"]) {
  await userEvent.click(await screen.findByRole("tab", { name: "Secrets" }));
  return screen.findByRole("table", { name: "Secrets" });
}

const row = (table: HTMLElement, name: string) => within(table).getByText(name).closest("tr") as HTMLElement;

describe("stack secrets tab", () => {
  it("lists items with their status", async () => {
    mockFetch(routes());
    const { screen } = renderApp("/envs/local/stacks/shop");
    const table = await openSecrets(screen);
    expect(within(row(table, "db_password")).getByText("Not set")).toBeInTheDocument();
    expect(within(row(table, "api_key")).getByText("Set")).toBeInTheDocument();
    expect(within(row(table, "old")).getByText("Not used")).toBeInTheDocument();
    expect(within(row(table, "var_conf")).getByText("variable CONF")).toBeInTheDocument();
    expect(within(row(table, "db_password")).getByText("db")).toBeInTheDocument();
  });

  it("sets a secret without ever prefilling it, and puts the value", async () => {
    const calls = mockFetch(routes());
    const { screen } = renderApp("/envs/local/stacks/shop");
    const table = await openSecrets(screen);
    await userEvent.click(within(row(table, "api_key")).getByRole("button", { name: "Replace" }));
    const replace = await screen.findByRole("dialog", { name: "Replace secret api_key" });
    expect(within(replace).getByLabelText("Value")).toHaveValue("");
    await userEvent.click(within(replace).getByRole("button", { name: "Cancel" }));
    await userEvent.click(within(row(table, "db_password")).getByRole("button", { name: "Set" }));
    const dialog = await screen.findByRole("dialog", { name: "Set secret db_password" });
    const input = within(dialog).getByLabelText("Value");
    expect(input).toHaveValue("");
    expect(input).toHaveAttribute("type", "password");
    await userEvent.type(input, "s3cr3t");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    const put = calls.find((c) => c.method === "PUT" && c.path.endsWith("/files/secret/db_password"));
    expect(put?.body).toEqual({ value: "s3cr3t" });
    expect(document.body.innerHTML).not.toContain("s3cr3t");
  });

  it("edits an inline stack's config", async () => {
    const calls = mockFetch(routes());
    const { screen } = renderApp("/envs/local/stacks/shop");
    const table = await openSecrets(screen);
    await userEvent.click(within(row(table, "app")).getByRole("button", { name: "Edit" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit config app" });
    const text = within(dialog).getByLabelText("Content");
    expect(text).toHaveValue("listen 80;");
    await userEvent.clear(text);
    await userEvent.type(text, "listen 8080;");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls.some((c) => c.method === "PUT" && c.path.endsWith("/files/config/app"))).toBe(true));
    const put = calls.find((c) => c.method === "PUT" && c.path.endsWith("/files/config/app"));
    expect(put?.body).toEqual({ value: "listen 8080;" });
  });

  it("shows a repo config read-only with a link to the file", async () => {
    const files = [item({ kind: "config", name: "site", source: "repo", path: "deploy/site.conf", set: true, content: "x" })];
    mockFetch({
      ...routes(files, "git"),
      "GET /api/v1/envs/local/stacks/shop/git": {
        json: {
          git: {
            stack_id: 1, url: "https://github.com/acme/ops.git", branch: "main", path: "deploy/compose.yaml", sync: "manual",
            poll_seconds: 300, auto_deploy: false, paused: false, has_token: false, last_fetched_commit: null,
            pending_commit: null, last_sync_at: null, last_sync_error: null, updated_at: 1,
          },
          webhook_url: "/api/v1/stacks/hooks/1",
        },
      },
    });
    const { screen } = renderApp("/envs/local/stacks/shop");
    const table = await openSecrets(screen);
    const r = row(table, "site");
    expect(await within(r).findByRole("link", { name: "deploy/site.conf" })).toHaveAttribute(
      "href",
      "https://github.com/acme/ops/blob/main/deploy/site.conf",
    );
    expect(within(r).queryByRole("button")).not.toBeInTheDocument();
  });

  it("is read-only without stacks.create", async () => {
    mockFetch({ ...routes(), ...withPerms(["stacks.view"]) });
    const { screen } = renderApp("/envs/local/stacks/shop");
    const table = await openSecrets(screen);
    expect(within(table).queryByRole("button")).not.toBeInTheDocument();
  });

  it("links a missing value in the deploy review to the tab", async () => {
    mockFetch({
      ...routes(),
      "POST /api/v1/envs/local/stacks/shop/plan": {
        json: plan([{ path: "secrets.db_password", message: "secret db_password has no value: set it in the Secrets tab" }]),
      },
    });
    const { screen } = renderApp("/envs/local/stacks/shop");
    await userEvent.click(await screen.findByRole("button", { name: "Review deploy" }));
    const dialog = await screen.findByRole("dialog", { name: "Deploy shop" });
    await userEvent.click(await within(dialog).findByRole("button", { name: "Secrets tab" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(await screen.findByRole("table", { name: "Secrets" })).toBeInTheDocument();
  });
});

describe("revision files", () => {
  it("lists the files a revision used, by name", async () => {
    const rev = {
      id: 1, stack_id: 1, number: 1, compose: COMPOSE, git_commit: null, status: "superseded",
      error: null, kind: "deploy", rollback_of: null, created_at: 4, created_by: 1,
    };
    mockFetch({
      ...routes(),
      "GET /api/v1/envs/local/stacks/shop/revisions": { json: [rev] },
      "GET /api/v1/envs/local/stacks/shop/revisions/1": {
        json: { revision: rev, variables: [], files: [{ kind: "config", name: "app" }, { kind: "secret", name: "db_password" }] },
      },
    });
    const { screen } = renderApp("/envs/local/stacks/shop");
    await userEvent.click(await screen.findByRole("tab", { name: "Revisions" }));
    const table = await screen.findByRole("table", { name: "Revisions" });
    await userEvent.click(within(table).getByRole("button", { name: "View" }));
    const dialog = await screen.findByRole("dialog", { name: "Revision 1" });
    expect(await within(dialog).findByText("config app, secret db_password")).toBeInTheDocument();
  });
});
