import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { mockFetch } from "@/test/fetch";
import { ENVS, loggedIn, renderApp } from "@/test/app";
import { repoFileLink } from "./StackEditor";

const COMPOSE = "services:\n  web:\n    image: nginx:1.27\n";
const SHA = "0123456789abcdef0123456789abcdef01234567";

const gitRecord = {
  stack_id: 1,
  url: "https://github.com/acme/ops.git",
  branch: "main",
  path: "deploy/compose.yaml",
  sync: "webhook",
  poll_seconds: 300,
  auto_deploy: false,
  paused: false,
  has_token: true,
  has_key: false,
  ssh_public_key: null as string | null,
  ssh_host_key: null as string | null,
  last_fetched_commit: SHA,
  pending_commit: null as string | null,
  last_sync_at: 10,
  last_sync_error: null as string | null,
  updated_at: 1,
};

const status = (g: Partial<typeof gitRecord> = {}) => ({
  git: { ...gitRecord, ...g },
  webhook_url: "/api/v1/stacks/hooks/1",
});

const detail = {
  stack: { id: 1, env_id: 1, name: "shop", source: "git", deployed_revision: 1, created_at: 0, created_by: 1 },
  draft: { compose: COMPOSE, variables: ["A=1"], updated_at: 5 },
  deployed: {
    id: 1, stack_id: 1, number: 1, compose: COMPOSE, git_commit: SHA, status: "deployed",
    error: null, kind: "deploy", rollback_of: null, created_at: 4, created_by: null,
  },
};

const plan = {
  issues: [],
  warnings: [],
  plan: { services: [], networks: [], volumes: [], images: [], secrets: [], configs: [], blocking: [], token: "tok" },
  compose_before: COMPOSE,
  compose_after: COMPOSE,
  variables_changed: [],
};

const routes = (git = status()) => ({
  ...loggedIn,
  "GET /api/v1/envs/local/stacks/shop": { json: detail },
  "GET /api/v1/envs/local/stacks/shop/git": { json: git },
  "POST /api/v1/envs/local/stacks/validate": { json: { issues: [], warnings: [] } },
  "PUT /api/v1/envs/local/stacks/shop/draft": { status: 204 },
  "POST /api/v1/envs/local/stacks/shop/plan": { json: plan },
  "GET /api/v1/envs/local/stacks/shop/revisions": { json: [detail.deployed] },
  "POST /api/v1/envs/local/stacks/shop/git/sync": { json: { outcome: "up_to_date", commit: SHA } },
  "POST /api/v1/envs/local/stacks/shop/git/resume": { json: { outcome: "pending", commit: SHA } },
});

const withPerms = (permissions: string[]) => ({
  "GET /api/v1/envs": {
    json: ENVS.map((e) => ({ ...e, access: { role: "operator", namespaces: null, grants: [{ namespaces: null, permissions }] } })),
  },
});

async function openGitTab(screen: ReturnType<typeof renderApp>["screen"]) {
  await userEvent.click(await screen.findByRole("tab", { name: "Git" }));
}

describe("git-backed stacks", () => {
  it("creates a stack from git, attaches it and shows the secret once", async () => {
    const calls = mockFetch({
      ...routes(),
      "GET /api/v1/envs/local/stacks": { json: [] },
      "GET /api/v1/envs/local/compose-projects": { json: [] },
      "POST /api/v1/envs/local/stacks": {
        status: 201,
        json: { name: "shop", source: "inline", deployed_revision: null, last_status: null, updated_at: 0, draft_differs: true, update_available: false, paused: false, sync_error: null },
      },
      "PUT /api/v1/envs/local/stacks/shop/git": { json: { status: status(), webhook_secret: "s3cr3t-hook" } },
    });
    const { screen } = renderApp("/envs/local/stacks");
    await userEvent.click(await screen.findByRole("button", { name: "New stack" }));
    const dialog = await screen.findByRole("dialog", { name: "New stack" });
    await userEvent.type(within(dialog).getByLabelText("Name"), "shop");
    await userEvent.click(within(dialog).getByLabelText("From a git repository"));
    await userEvent.type(within(dialog).getByLabelText("Repository URL"), "https://github.com/acme/ops.git");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    const secret = await screen.findByRole("dialog", { name: "Webhook secret" });
    expect(within(secret).getByLabelText("Webhook secret")).toHaveTextContent("s3cr3t-hook");
    const created = calls.find((c) => c.method === "POST" && c.path === "/api/v1/envs/local/stacks");
    expect(created?.body).toMatchObject({ name: "shop", compose: "" });
    const put = calls.find((c) => c.method === "PUT" && c.path.endsWith("/shop/git"));
    expect(put?.body).toMatchObject({ url: "https://github.com/acme/ops.git", branch: "main", sync: "manual" });
    await userEvent.click(within(secret).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Webhook secret" })).not.toBeInTheDocument());
    expect(screen.queryByText("s3cr3t-hook")).not.toBeInTheDocument();
  });

  it("never prefills a saved token", async () => {
    mockFetch(routes());
    const { screen } = renderApp("/envs/local/stacks/shop");
    await openGitTab(screen);
    expect(await screen.findByText("Token saved")).toBeInTheDocument();
    expect(screen.queryByLabelText("Access token", { selector: "input" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Replace" }));
    expect(screen.getByLabelText("Access token")).toHaveValue("");
  });

  it("shows a read-only file with a link to the repo, and editable variables", async () => {
    const calls = mockFetch(routes());
    const { screen } = renderApp("/envs/local/stacks/shop");
    const link = await screen.findByRole("link", { name: /github\.com\/acme\/ops\.git @ main: deploy\/compose\.yaml/ });
    expect(link).toHaveAttribute("href", "https://github.com/acme/ops/blob/main/deploy/compose.yaml");
    expect(screen.getAllByLabelText("Compose file")[0].tagName).toBe("PRE");
    const value = screen.getByLabelText("Variable 1 value");
    expect(value).toBeEnabled();
    await userEvent.clear(value);
    await userEvent.type(value, "2");
    await userEvent.click(screen.getByRole("button", { name: "Save draft" }));
    await waitFor(() => expect(calls.some((c) => c.method === "PUT" && c.path.endsWith("/draft"))).toBe(true));
    const put = calls.find((c) => c.method === "PUT" && c.path.endsWith("/draft"));
    expect(put?.body).toEqual({ compose: COMPOSE, variables: ["A=2"] });
  });

  it("offers Resume when paused, and calls the API", async () => {
    const calls = mockFetch(routes(status({ paused: true })));
    const { screen } = renderApp("/envs/local/stacks/shop");
    await openGitTab(screen);
    expect(await screen.findByText("Auto-sync paused after rollback.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Resume" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path.endsWith("/git/resume"))).toBe(true));
  });

  it("reviews an available update as the draft", async () => {
    const calls = mockFetch(routes(status({ pending_commit: SHA })));
    const { screen } = renderApp("/envs/local/stacks/shop");
    await openGitTab(screen);
    const section = await screen.findByRole("region", { name: "Sync" });
    expect(within(section).getByText(/Update available/)).toHaveTextContent("Update available: 0123456");
    await userEvent.click(within(section).getByRole("button", { name: "Review deploy" }));
    await screen.findByRole("dialog", { name: "Deploy shop" });
    const p = calls.find((c) => c.method === "POST" && c.path.endsWith("/shop/plan"));
    expect(p?.body).toEqual({ source: "draft" });
  });

  it("hides Sync now without operate, and locks settings without create", async () => {
    mockFetch({ ...routes(), ...withPerms(["stacks.view"]) });
    const { screen } = renderApp("/envs/local/stacks/shop");
    await openGitTab(screen);
    expect(await screen.findByLabelText("Repository URL")).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Sync now" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Rotate secret" })).not.toBeInTheDocument();
  });

  it("badges git stacks in the list", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/compose-projects": { json: [] },
      "GET /api/v1/envs/local/stacks": {
        json: [
          { name: "shop", source: "git", deployed_revision: 1, last_status: "deployed", updated_at: 0, draft_differs: true, update_available: true, paused: true, sync_error: "branch 'x' not found" },
        ],
      },
    });
    const { screen } = renderApp("/envs/local/stacks");
    const table = await screen.findByRole("table", { name: "Stacks" });
    expect(await within(table).findByText("Update available")).toBeInTheDocument();
    expect(within(table).getByText("Paused")).toBeInTheDocument();
    expect(within(table).getByText("Sync error")).toHaveAttribute("title", "branch 'x' not found");
    expect(within(table).queryByText("Update pending")).not.toBeInTheDocument();
  });

  it("shows the commit of each revision", async () => {
    mockFetch(routes());
    const { screen } = renderApp("/envs/local/stacks/shop");
    await userEvent.click(await screen.findByRole("tab", { name: "Revisions" }));
    const table = await screen.findByRole("table", { name: "Revisions" });
    expect(within(table).getByRole("columnheader", { name: "Commit" })).toBeInTheDocument();
    expect(within(table).getByText("0123456")).toBeInTheDocument();
  });

  it("swaps the token for a generated deploy key on an ssh URL", async () => {
    const calls = mockFetch({
      ...routes(),
      "PUT /api/v1/envs/local/stacks/shop/git": { json: { status: status(), webhook_secret: null } },
    });
    const { screen } = renderApp("/envs/local/stacks/shop");
    await openGitTab(screen);
    const url = await screen.findByLabelText("Repository URL");
    await userEvent.clear(url);
    await userEvent.type(url, "git@git.acme:ops/shop.git");
    expect(screen.queryByText("Token saved")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Access token")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Deploy key")).toHaveValue("generate");
    expect(screen.getByText("Add the public key to the repository as a read-only deploy key.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls.some((c) => c.method === "PUT" && c.path.endsWith("/shop/git"))).toBe(true));
    const put = calls.find((c) => c.method === "PUT" && c.path.endsWith("/shop/git"));
    expect(put?.body).toMatchObject({ url: "git@git.acme:ops/shop.git", generate_key: true, ssh_key: null, token: null });
  });

  it("sends a pasted private key", async () => {
    const calls = mockFetch({
      ...routes(status({ url: "git@git.acme:ops/shop.git", has_token: false })),
      "PUT /api/v1/envs/local/stacks/shop/git": { json: { status: status(), webhook_secret: null } },
    });
    const { screen } = renderApp("/envs/local/stacks/shop");
    await openGitTab(screen);
    await userEvent.selectOptions(await screen.findByLabelText("Deploy key"), "paste");
    await userEvent.type(screen.getByLabelText("Private key"), "-----BEGIN OPENSSH PRIVATE KEY-----");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls.some((c) => c.method === "PUT" && c.path.endsWith("/shop/git"))).toBe(true));
    const put = calls.find((c) => c.method === "PUT" && c.path.endsWith("/shop/git"));
    expect(put?.body).toMatchObject({ generate_key: false, ssh_key: "-----BEGIN OPENSSH PRIVATE KEY-----" });
  });

  it("shows a stored key's public key and the pinned host key", async () => {
    mockFetch(
      routes(
        status({
          url: "git@git.acme:ops/shop.git",
          has_token: false,
          has_key: true,
          ssh_public_key: "ssh-ed25519 AAAAC3 fleetops:shop",
          ssh_host_key: "ssh-ed25519 SHA256:abc",
        }),
      ),
    );
    const { screen } = renderApp("/envs/local/stacks/shop");
    await openGitTab(screen);
    expect(await screen.findByLabelText("Public key")).toHaveTextContent("ssh-ed25519 AAAAC3 fleetops:shop");
    expect(screen.getByRole("button", { name: "Copy public key" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove key" })).toBeInTheDocument();
    expect(screen.getByText("ssh-ed25519 SHA256:abc (trusted on first sync)")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Accept new key" })).not.toBeInTheDocument();
  });

  it("accepts a changed host key, then syncs", async () => {
    const calls = mockFetch({
      ...routes(
        status({
          url: "ssh://git@git.acme/ops/shop.git",
          has_key: true,
          ssh_public_key: "ssh-ed25519 AAAAC3 fleetops:shop",
          ssh_host_key: "ssh-ed25519 SHA256:abc",
          last_sync_error:
            "the host key of git.acme changed (now SHA256:new); if that is expected, accept the new key in the Git tab",
        }),
      ),
      "DELETE /api/v1/envs/local/stacks/shop/git/host-key": { status: 204 },
    });
    const { screen } = renderApp("/envs/local/stacks/shop");
    await openGitTab(screen);
    await userEvent.click(await screen.findByRole("button", { name: "Accept new key" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path.endsWith("/git/sync"))).toBe(true));
    const del = calls.findIndex((c) => c.method === "DELETE" && c.path.endsWith("/git/host-key"));
    const sync = calls.findIndex((c) => c.method === "POST" && c.path.endsWith("/git/sync"));
    expect(del).toBeGreaterThanOrEqual(0);
    expect(del).toBeLessThan(sync);
  });

  it("offers no accept for other sync errors", async () => {
    mockFetch(routes(status({ url: "git@git.acme:ops/shop.git", last_sync_error: "the repository refused the SSH key" })));
    const { screen } = renderApp("/envs/local/stacks/shop");
    await openGitTab(screen);
    expect(await screen.findByText("the repository refused the SSH key")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Accept new key" })).not.toBeInTheDocument();
  });

  it("links ssh URLs of known hosts to their file pages", () => {
    expect(repoFileLink("git@github.com:acme/ops.git", "main", "c.yaml")).toBe("https://github.com/acme/ops/blob/main/c.yaml");
    expect(repoFileLink("ssh://git@gitlab.com/acme/ops.git", "main", "c.yaml")).toBe(
      "https://gitlab.com/acme/ops/-/blob/main/c.yaml",
    );
    expect(repoFileLink("ssh://git@github.com:22/acme/ops.git", "dev", "a/b.yml")).toBe(
      "https://github.com/acme/ops/blob/dev/a/b.yml",
    );
    expect(repoFileLink("git@git.acme:ops.git", "main", "c.yaml")).toBeNull();
  });
});
