import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { User } from "@/api/gen/types.gen";
import { isEnterpriseBuild } from "@/ee-slots";
import { json, mockFetch } from "@/test/fetch";
import { ADMIN, ENVS, KUBE_ENV, loggedIn, openRowMenu, pickRowMenu, renderApp } from "@/test/app";

const BOB: User = { id: 2, username: "bob", role: "user", created_at: 1_790_000_000, updated_at: 0 };
const routes = { ...loggedIn, "GET /api/v1/users": { json: [ADMIN.user, BOB] } };

const row = (screen: typeof import("@testing-library/react").screen, name: string) =>
  within(screen.getByRole("table", { name: /Users|Environments/ }))
    .getAllByRole("row")
    .find((r) => r.textContent?.includes(name))!;

describe("admin access", () => {
  it("shows the administration links to admins only", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local");
    const nav = await screen.findByRole("navigation", { name: "Administration" });
    expect(within(nav).getByRole("link", { name: "Users" })).toHaveAttribute("href", "/admin/users");
  });

  it("sends a user who is not an admin away", async () => {
    mockFetch({ ...routes, "GET /api/v1/auth/me": { json: { ...ADMIN, user: BOB } } });
    const { screen, history } = renderApp("/admin/users");
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local"));
    expect(screen.queryByRole("navigation", { name: "Administration" })).not.toBeInTheDocument();
  });
});

describe("users", () => {
  it("marks single sign-on users and locks their role and password", async () => {
    mockFetch({ ...routes, "GET /api/v1/users": { json: [ADMIN.user, { ...BOB, sso: "Acme Okta" }] } });
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "bob")).toHaveTextContent("Signed in with Acme Okta"));
    const items = async (name: string) => within(await openRowMenu(name)).getAllByRole("menuitem").map((m) => m.textContent);
    expect(await items("bob")).toEqual(["Disable and sign out", "Delete"]);
    await userEvent.keyboard("{Escape}");
    expect(await items("admin")).toEqual(["Change role", "Reset password", "Delete"]);
  });

  it("lists users and marks the current one", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "admin")).toHaveTextContent("(you)"));
    expect(row(screen, "bob")).toHaveTextContent("User");
  });

  it("adds a user", async () => {
    let users = [ADMIN.user, BOB];
    const calls = mockFetch({
      ...routes,
      "GET /api/v1/users": () => json(200, users),
      "POST /api/v1/users": (c) => {
        const u = { ...BOB, id: 3, ...(c.body as object) } as User;
        users = [...users, u];
        return json(201, u);
      },
    });
    const { screen } = renderApp("/admin/users");
    await userEvent.click(await screen.findByRole("button", { name: "Add user" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Username"), "carol");
    await userEvent.type(within(dialog).getByLabelText("Password"), "a-long-password");
    await userEvent.selectOptions(within(dialog).getByLabelText("Role"), "admin");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() => expect(row(screen, "carol")).toBeTruthy());
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      username: "carol",
      password: "a-long-password",
      role: "admin",
    });
  });

  it("shows the server's reason a user cannot be added", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/users": () => json(400, { code: "invalid", message: "password must be 12-128 characters" }),
    });
    const { screen } = renderApp("/admin/users");
    await userEvent.click(await screen.findByRole("button", { name: "Add user" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Username"), "carol");
    await userEvent.type(within(dialog).getByLabelText("Password"), "short");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("password must be 12-128 characters");
  });

  it("changes a role and resets a password", async () => {
    const calls = mockFetch({ ...routes, "PATCH /api/v1/users/2": { json: BOB } });
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "bob")).toBeTruthy());
    await pickRowMenu("bob", "Change role");
    let dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByLabelText("Role"), "admin");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await pickRowMenu("bob", "Reset password");
    dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("New password"), "another-long-one");
    await userEvent.click(within(dialog).getByRole("button", { name: "Reset password" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.filter((c) => c.method === "PATCH").map((c) => c.body)).toEqual([
      { role: "admin" },
      { password: "another-long-one" },
    ]);
  });

  it("disables a user after confirming, and enables them again from the menu", async () => {
    let bob: User = BOB;
    const calls = mockFetch({
      ...routes,
      "GET /api/v1/users": () => json(200, [ADMIN.user, bob]),
      "PATCH /api/v1/users/2": (c) => {
        bob = { ...bob, disabled_at: (c.body as { disabled: boolean }).disabled ? 1_790_000_100 : null };
        return json(200, bob);
      },
    });
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "bob")).toBeTruthy());
    expect(within(await openRowMenu("admin")).queryByRole("menuitem", { name: "Disable and sign out" })).toBeNull();
    await userEvent.keyboard("{Escape}");
    await pickRowMenu("bob", "Disable and sign out");
    const dialog = await screen.findByRole("dialog", { name: "Disable bob?" });
    expect(dialog).toHaveTextContent("signed out everywhere");
    await userEvent.click(within(dialog).getByRole("button", { name: "Disable and sign out" }));
    await waitFor(() => expect(row(screen, "bob")).toHaveTextContent("disabled"));
    await pickRowMenu("bob", "Enable");
    await waitFor(() => expect(row(screen, "bob")).not.toHaveTextContent("disabled"));
    expect(calls.filter((c) => c.method === "PATCH").map((c) => c.body)).toEqual([{ disabled: true }, { disabled: false }]);
  });

  it("refuses to delete the last admin with the server's reason", async () => {
    mockFetch({
      ...routes,
      "DELETE /api/v1/users/1": () => json(409, { code: "conflict", message: "cannot delete the last admin" }),
    });
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "admin")).toBeTruthy());
    await pickRowMenu("admin", "Delete");
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("This is your own account");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("cannot delete the last admin");
  });
});

describe("environments", () => {
  it("lists environments with their status", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "local")).toHaveTextContent("up"));
    expect(row(screen, "local")).toHaveTextContent("unix:///var/run/docker.sock");
  });

  it("adds an environment", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs": { status: 201, json: { ...ENVS[0], id: 2, name: "remote", url: "tcp://10.0.0.5:2375" } },
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "remote");
    await userEvent.clear(within(dialog).getByLabelText("Docker URL"));
    await userEvent.type(within(dialog).getByLabelText("Docker URL"), "tcp://10.0.0.5:2375");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      name: "remote",
      url: "tcp://10.0.0.5:2375",
      kind: "docker-local",
    });
  });

  it("renames an environment, sending only what changed", async () => {
    const calls = mockFetch({ ...routes, "PATCH /api/v1/envs/local": { json: { ...ENVS[0], name: "laptop" } } });
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "local")).toBeTruthy());
    await pickRowMenu("local", "Edit");
    const dialog = await screen.findByRole("dialog");
    await userEvent.clear(within(dialog).getByLabelText("Name"));
    await userEvent.type(within(dialog).getByLabelText("Name"), "laptop");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ name: "laptop", url: null });
  });

  it("deletes an environment after confirming", async () => {
    const calls = mockFetch({ ...routes, "DELETE /api/v1/envs/local": { status: 204 } });
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "local")).toBeTruthy());
    await pickRowMenu("local", "Delete");
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE")).toBe(true));
  });
});

describe("agent environments", () => {
  const AGENT = {
    id: 2,
    name: "web-01",
    kind: "docker-agent",
    url: "agent://2",
    created_at: 0,
    updated_at: 0,
    agent: {
      connected: false,
      enrolled: true,
      last_seen_at: Math.floor(Date.now() / 1000) - 600,
      agent_version: "0.2.0",
      host: { hostname: "web-01" },
      last_error: null,
      join_expires_at: null,
    },
  };
  const LOCAL = { ...ENVS[0], agent: null };
  const enrollment = (token: string) => ({
    join_token: token,
    expires_at: Math.floor(Date.now() / 1000) + 900,
    command: `docker run … --join-token ${token}`,
  });
  const agentRoutes = {
    ...routes,
    "GET /api/v1/envs": { json: [LOCAL, AGENT] },
    "GET /api/v1/envs/web-01/status": { json: { state: "down", reason: "agent offline: connection lost" } },
  };

  it("adds an agent environment and shows its one-time command", async () => {
    const calls = mockFetch({
      ...agentRoutes,
      "POST /api/v1/envs": { status: 201, json: { ...AGENT, agent: undefined, enrollment: enrollment("fj_abc") } },
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("radio", { name: /agent on a remote host/i }));
    expect(within(dialog).queryByLabelText("Docker URL")).not.toBeInTheDocument();
    await userEvent.type(within(dialog).getByLabelText("Name"), "web-01");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ name: "web-01", kind: "docker-agent" });
    const shown = await screen.findByRole("dialog", { name: /run this on the remote host/i });
    expect(within(shown).getByText(/--join-token fj_abc/)).toBeInTheDocument();
    expect(within(shown).getByRole("button", { name: /copy/i })).toBeInTheDocument();
    expect(within(shown).getByText(/expires in 1[45] min/i)).toBeInTheDocument();
    await userEvent.click(within(shown).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("shows agent state and offers rotate only for agents", async () => {
    mockFetch(agentRoutes);
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "web-01")).toBeTruthy());
    expect(row(screen, "web-01")).toHaveTextContent("0.2.0");
    expect(row(screen, "web-01")).toHaveTextContent("last seen 10 minutes ago");
    expect(within(await openRowMenu("web-01")).getByRole("menuitem", { name: "Rotate key" })).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(within(await openRowMenu("local")).queryByRole("menuitem", { name: "Rotate key" })).not.toBeInTheDocument();
  });

  it("rotating asks first, then shows the new command", async () => {
    const calls = mockFetch({ ...agentRoutes, "POST /api/v1/envs/web-01/agent/rotate": { json: enrollment("fj_new") } });
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "web-01")).toBeTruthy());
    await pickRowMenu("web-01", "Rotate key");
    const confirm = await screen.findByRole("dialog");
    expect(confirm).toHaveTextContent(/disconnects the running agent/i);
    await userEvent.click(within(confirm).getByRole("button", { name: "Rotate" }));
    expect(await screen.findByText(/--join-token fj_new/)).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST" && c.path.endsWith("/agent/rotate"))).toBe(true);
  });

  it("an agent's URL is not editable", async () => {
    mockFetch(agentRoutes);
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "web-01")).toBeTruthy());
    await pickRowMenu("web-01", "Edit");
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByLabelText("Docker URL")).not.toBeInTheDocument();
  });
});

describe("Kubernetes environments", () => {
  const KUBECONFIG = "apiVersion: v1\nkind: Config\ncontexts: [{name: dev}, {name: prod}]\n";

  it("adds a kubeconfig cluster after picking its context", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/kube/contexts": { json: { contexts: ["dev", "prod"], current: "dev" } },
      "POST /api/v1/envs": { status: 201, json: { id: 9, name: "k", kind: "kube-config", url: "https://k:6443", created_at: 1, updated_at: 1, enrollment: null } },
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("radio", { name: "Kubernetes" }));
    await userEvent.click(within(dialog).getByRole("radio", { name: /kubeconfig/i }));
    expect(within(dialog).queryByLabelText("Docker URL")).not.toBeInTheDocument();
    await userEvent.type(within(dialog).getByLabelText("Name"), "k");
    await userEvent.click(within(dialog).getByLabelText("Kubeconfig"));
    await userEvent.paste(KUBECONFIG);
    await userEvent.tab();
    const ctx = await within(dialog).findByRole("combobox", { name: "Context" });
    expect(ctx).toHaveValue("dev");
    await userEvent.selectOptions(ctx, "prod");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    expect(calls.find((c) => c.method === "POST" && c.path === "/api/v1/envs")?.body).toEqual({
      name: "k", kind: "kube-config", kubeconfig: KUBECONFIG, context: "prod", skip_check: false,
    });
  });

  it("shows an unreachable cluster's error inline and can save without the check", async () => {
    let attempt = 0;
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/kube/contexts": { json: { contexts: ["dev"], current: "dev" } },
      "POST /api/v1/envs": () =>
        ++attempt === 1
          ? json(400, { code: "invalid", message: "invalid input: cannot reach cluster: timed out", request_id: null })
          : json(201, { id: 9, name: "k", kind: "kube-config", url: "https://k", created_at: 1, updated_at: 1, enrollment: null }),
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("radio", { name: "Kubernetes" }));
    await userEvent.click(within(dialog).getByRole("radio", { name: /kubeconfig/i }));
    await userEvent.type(within(dialog).getByLabelText("Name"), "k");
    await userEvent.click(within(dialog).getByLabelText("Kubeconfig"));
    await userEvent.paste(KUBECONFIG);
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    expect(await within(dialog).findByText(/cannot reach cluster/)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("checkbox", { name: /without checking/i }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    const posts = calls.filter((c) => c.method === "POST" && c.path === "/api/v1/envs");
    expect(posts[1].body).toMatchObject({ skip_check: true });
  });

  it("adds an agent cluster and shows the kubectl command", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs": { status: 201, json: { id: 9, name: "k8s-01", kind: "kube-agent", url: "agent://9", created_at: 1, updated_at: 1,
        enrollment: { join_token: "fj_abc", expires_at: Math.floor(Date.now() / 1000) + 900, command: "kubectl apply -f - <<'EOF'\n…\nEOF\n" } } },
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("radio", { name: "Kubernetes" }));
    await userEvent.click(within(dialog).getByRole("radio", { name: /in-cluster agent/i }));
    await userEvent.type(within(dialog).getByLabelText("Name"), "k8s-01");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    const shown = await screen.findByRole("dialog", { name: /run this against the cluster/i });
    expect(within(shown).getByText(/kubectl apply/)).toBeInTheDocument();
  });

  it("asks for an agent cluster's access level and sends it", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs": { status: 201, json: { id: 9, name: "ka", kind: "kube-agent", url: "agent://9", created_at: 1, updated_at: 1,
        enrollment: { join_token: "fj_abc", expires_at: Math.floor(Date.now() / 1000) + 900, command: "kubectl apply" } } },
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("radio", { name: "Kubernetes" }));
    await userEvent.click(within(dialog).getByRole("radio", { name: /in-cluster agent/i }));
    expect(within(dialog).getByRole("radio", { name: /^Standard/ })).toBeChecked();
    await userEvent.click(within(dialog).getByRole("radio", { name: /^Cluster admin/ }));
    expect(within(dialog).getByText(/full control of the cluster/)).toBeInTheDocument();
    await userEvent.type(within(dialog).getByLabelText("Name"), "ka");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "POST" && c.path === "/api/v1/envs")?.body).toMatchObject({ kind: "kube-agent", access: "cluster-admin" }),
    );
  });

  it("lets an agent cluster read custom resources, off unless asked", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs": { status: 201, json: { id: 9, name: "kc", kind: "kube-agent", url: "agent://9", created_at: 1, updated_at: 1,
        enrollment: { join_token: "fj_abc", expires_at: Math.floor(Date.now() / 1000) + 900, command: "kubectl apply" } } },
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("radio", { name: "Kubernetes" }));
    await userEvent.click(within(dialog).getByRole("radio", { name: /in-cluster agent/i }));
    const box = within(dialog).getByRole("checkbox", { name: /Read custom resources/ });
    expect(box).not.toBeChecked();
    expect(within(dialog).getByText(/Secrets included/)).toBeInTheDocument();
    await userEvent.click(box);
    await userEvent.type(within(dialog).getByLabelText("Name"), "kc");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "POST" && c.path === "/api/v1/envs")?.body).toMatchObject({ kind: "kube-agent", custom_resources: true }),
    );
  });

  it("names the exec plugin a kubeconfig cluster signs in with", async () => {
    mockFetch({
      ...routes,
      "GET /api/v1/envs": { json: [{ id: 9, name: "eks-1", kind: "kube-config", url: "https://k:6443", created_at: 1, updated_at: 1, agent: null,
        kube: { server: "https://k:6443", context: "prod", user_kind: "exec", exec_command: "aws" } }] },
    });
    const { screen } = renderApp("/admin/envs");
    await within(await screen.findByRole("table", { name: "Environments" })).findByText("eks-1");
    expect(within(row(screen, "eks-1")).getByText(/via aws/)).toBeInTheDocument();
  });

  it("lists a kubeconfig cluster's server and context", async () => {
    mockFetch({
      ...routes,
      "GET /api/v1/envs": { json: [{ id: 9, name: "cluster-1", kind: "kube-config", url: "https://k:6443", created_at: 1, updated_at: 1, agent: null,
        kube: { server: "https://k:6443", context: "prod", user_kind: "token" } }] },
    });
    const { screen } = renderApp("/admin/envs");
    await within(await screen.findByRole("table", { name: "Environments" })).findByText("cluster-1");
    const r = row(screen, "cluster-1");
    expect(within(r).getByText(/prod/)).toBeInTheDocument();
    expect(within(r).getByText(/token/)).toBeInTheDocument();
  });
});

describe("access", () => {
  it("grants one environment to several users at once, naming the ones that failed", async () => {
    const CAROL: User = { ...BOB, id: 3, username: "carol" };
    const calls = mockFetch({
      ...routes,
      "GET /api/v1/users": { json: [ADMIN.user, BOB, CAROL] },
      "PUT /api/v1/users/2/access/1": { json: { env_id: 1, env_name: "local", env_kind: "docker-local", role: "operator" } },
      "PUT /api/v1/users/3/access/1": () => json(403, { code: "forbidden", message: "seat limit reached" }),
    });
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "carol")).toBeTruthy());
    await userEvent.click(within(row(screen, "bob")).getByRole("checkbox"));
    await userEvent.click(within(row(screen, "carol")).getByRole("checkbox"));
    await userEvent.click(screen.getByRole("button", { name: "Access 2" }));
    const dialog = await screen.findByRole("dialog", { name: "Access for 2 users" });
    await userEvent.selectOptions(await within(dialog).findByLabelText("Role on local"), "operator");
    await userEvent.click(within(dialog).getByRole("button", { name: "Grant access" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Not saved for carol (seat limit reached). The other 1 saved.");
    expect(calls.filter((c) => c.method === "PUT").map((c) => c.path)).toEqual([
      "/api/v1/users/2/access/1",
      "/api/v1/users/3/access/1",
    ]);
  });

  it("grants a namespace-limited role from the Access dialog", async () => {
    const calls = mockFetch({
      ...routes,
      "GET /api/v1/envs": { json: [...ENVS, KUBE_ENV] },
      "GET /api/v1/users/2/access": { json: [] },
      "PUT /api/v1/users/2/access/2": {
        json: { env_id: 2, env_name: "k", env_kind: "kube-config", role: "operator", namespaces: ["team-a"] },
      },
    });
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "bob")).toBeTruthy());
    await userEvent.click(within(row(screen, "bob")).getByRole("button", { name: "Access" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(await within(dialog).findByLabelText("Role on k"), "operator");
    await userEvent.click(within(dialog).getByLabelText("Entire cluster"));
    await userEvent.type(within(dialog).getByLabelText("Namespaces on k"), "team-a{enter}");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.filter((c) => c.method === "PUT").map((c) => [c.path, c.body])).toEqual([
      ["/api/v1/users/2/access/2", { role: "operator", namespaces: ["team-a"] }],
    ]);
  });

  it("limits a swarm grant to stacks", async () => {
    const SWARM = { id: 7, name: "prod", kind: "swarm-agent", url: "agent://7", created_at: 0, updated_at: 0 };
    const calls = mockFetch({
      ...routes,
      "GET /api/v1/envs": { json: [...ENVS, SWARM] },
      "GET /api/v1/users/2/access": { json: [] },
      "PUT /api/v1/users/2/access/7": {
        json: { env_id: 7, env_name: "prod", env_kind: "swarm-agent", role: "operator", namespaces: ["blog"] },
      },
    });
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "bob")).toBeTruthy());
    await userEvent.click(within(row(screen, "bob")).getByRole("button", { name: "Access" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(await within(dialog).findByLabelText("Role on prod"), "operator");
    await userEvent.click(within(dialog).getByLabelText("Entire swarm"));
    await userEvent.type(within(dialog).getByLabelText("Stacks on prod"), "blog{enter}");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.filter((c) => c.method === "PUT").map((c) => [c.path, c.body])).toEqual([
      ["/api/v1/users/2/access/7", { role: "operator", namespaces: ["blog"] }],
    ]);
  });

  it("revokes a grant by choosing None", async () => {
    const calls = mockFetch({
      ...routes,
      "GET /api/v1/users/2/access": {
        json: [{ env_id: 1, env_name: "local", env_kind: "docker-local", role: "viewer", namespaces: null }],
      },
      "DELETE /api/v1/users/2/access/1": { status: 204 },
    });
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "bob")).toBeTruthy());
    await userEvent.click(within(row(screen, "bob")).getByRole("button", { name: "Access" }));
    const dialog = await screen.findByRole("dialog");
    const role = await within(dialog).findByLabelText("Role on local");
    await waitFor(() => expect(role).toHaveValue("viewer"));
    await userEvent.selectOptions(role, "none");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE")).toBe(true));
  });

  it.runIf(!isEnterpriseBuild)("shows no team access in Community", async () => {
    const calls = mockFetch({ ...routes, "GET /api/v1/users/2/access": { json: [] } });
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "bob")).toBeTruthy());
    await userEvent.click(within(row(screen, "bob")).getByRole("button", { name: "Access" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByLabelText("Role on local");
    expect(within(dialog).queryByText("Through teams")).toBeNull();
    expect(calls.some((c) => c.path.startsWith("/api/v1/ee/"))).toBe(false);
  });

  it("has no Access button for admins", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "bob")).toBeTruthy());
    expect(within(row(screen, "admin")).queryByRole("button", { name: "Access" })).toBeNull();
    expect(within(row(screen, "bob")).getByRole("button", { name: "Access" })).toBeInTheDocument();
  });

  it("lists who can use an environment", async () => {
    mockFetch({
      ...routes,
      "GET /api/v1/envs/local/access": { json: [{ user_id: 2, username: "bob", role: "viewer", namespaces: null }] },
    });
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "local")).toBeTruthy());
    await userEvent.click(within(row(screen, "local")).getByRole("button", { name: "Access" }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText("bob")).toBeInTheDocument();
    expect(within(dialog).getByText("viewer")).toBeInTheDocument();
  });
});
