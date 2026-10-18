import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import type { SsoConnection } from "./sso";

const OKTA: SsoConnection = {
  id: 3,
  name: "Acme Okta",
  kind: "oidc",
  enabled: true,
  settings: { kind: "oidc", issuer: "https://idp.acme.io", client_id: "fleet", scopes: ["openid"], groups_claim: "groups" },
  secrets_set: ["client_secret"],
  require_mapped_group: false,
  session_hours: 12,
  callback_url: "https://fleet.acme.io/api/v1/ee/sso/3/callback",
  user_count: 4,
};

const LDAP: SsoConnection = {
  id: 5,
  name: "Corp directory",
  kind: "ldap",
  enabled: true,
  settings: {
    kind: "ldap",
    preset: "openldap",
    url: "ldap://dc.acme.io",
    starttls: false,
    skip_tls_verify: false,
    ca_cert: null,
    bind_dn: "cn=admin,dc=acme,dc=io",
    user_base_dn: "dc=acme,dc=io",
    user_filter: "(&(objectClass=inetOrgPerson)(uid={username}))",
    username_attr: "uid",
    email_attr: "mail",
    group_base_dn: null,
    group_filter: "",
  },
  secrets_set: ["bind_password"],
  require_mapped_group: false,
  session_hours: 12,
  callback_url: null,
  user_count: 0,
};

const routes = (features: string[]) => ({
  ...loggedIn,
  "GET /api/v1/edition": { json: { edition: "enterprise", features } },
  "GET /api/v1/ee/sso/connections": { json: [OKTA] },
  "GET /api/v1/ee/sso/settings": { json: { password_login: "everyone" } },
  "GET /api/v1/ee/sso/connections/3/mappings": {
    json: [{ id: 1, group: "ops", team_id: 7, team_name: "Ops", admin: false }],
  },
  "GET /api/v1/ee/teams": { json: [{ id: 7, name: "Ops", description: null, members: 0, environments: 0 }] },
});

const row = (screen: typeof import("@testing-library/react").screen, name: string) =>
  within(screen.getByRole("table", { name: "Connections" }))
    .getAllByRole("row")
    .find((r) => r.textContent?.includes(name))!;

describe("single sign-on", () => {
  it("creates a connection with fields for its kind", async () => {
    const calls = mockFetch({
      ...routes(["sso"]),
      "POST /api/v1/ee/sso/connections": { status: 201, json: { ...OKTA, id: 4, name: "GH", kind: "github" } },
    });
    const { screen } = renderApp("/admin/sso");
    await waitFor(() => expect(row(screen, "Acme Okta")).toHaveTextContent("OpenID Connect"));
    await userEvent.click(screen.getByRole("button", { name: "New connection" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByLabelText("Issuer URL")).toBeInTheDocument();
    await userEvent.selectOptions(within(dialog).getByLabelText("Provider"), "github");
    expect(within(dialog).queryByLabelText("Issuer URL")).toBeNull();
    await userEvent.type(within(dialog).getByLabelText("Name"), "GH");
    await userEvent.type(within(dialog).getByLabelText("Client ID"), "abc");
    await userEvent.type(within(dialog).getByLabelText("Client secret"), "shh");
    await userEvent.type(within(dialog).getByLabelText("Allowed organisations"), "acme, labs");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      name: "GH",
      settings: { kind: "github", client_id: "abc", orgs: ["acme", "labs"], base_url: null },
      secrets: { client_secret: "shh" },
      require_mapped_group: false,
      session_hours: 12,
    });
  });

  it("never prefills a saved secret and keeps it when left empty", async () => {
    const calls = mockFetch({ ...routes(["sso"]), "PATCH /api/v1/ee/sso/connections/3": { json: OKTA } });
    const { screen } = renderApp("/admin/sso");
    await waitFor(() => expect(row(screen, "Acme Okta")).toBeTruthy());
    await userEvent.click(within(row(screen, "Acme Okta")).getByRole("button", { name: "Edit" }));
    const dialog = await screen.findByRole("dialog");
    const secret = within(dialog).getByLabelText("Client secret");
    expect(secret).toHaveValue("");
    expect(secret).toHaveAttribute("placeholder", "Saved — leave empty to keep");
    expect(within(dialog).getByLabelText("Callback URL")).toHaveValue(OKTA.callback_url);
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls.some((c) => c.method === "PATCH")).toBe(true));
    expect((calls.find((c) => c.method === "PATCH")?.body as { secrets: object }).secrets).toEqual({});
  });

  it("maps a group to admin", async () => {
    const calls = mockFetch({
      ...routes(["sso"]),
      "POST /api/v1/ee/sso/connections/3/mappings": { status: 201, json: { id: 2, group: "admins", team_id: null, team_name: null, admin: true } },
    });
    const { screen } = renderApp("/admin/sso");
    await waitFor(() => expect(row(screen, "Acme Okta")).toBeTruthy());
    await userEvent.click(within(row(screen, "Acme Okta")).getByRole("button", { name: "Mappings" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("ops");
    await userEvent.type(within(dialog).getByLabelText("Group"), "admins");
    await userEvent.selectOptions(within(dialog).getByLabelText("Gives"), "admin");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "POST")?.body).toEqual({ group: "admins", admin: true }),
    );
  });

  it("without a license, disables adding but keeps disable and delete", async () => {
    mockFetch(routes([]));
    const { screen } = renderApp("/admin/sso");
    await waitFor(() => expect(row(screen, "Acme Okta")).toBeTruthy());
    expect(screen.getByRole("button", { name: "New connection" })).toBeDisabled();
    expect(within(row(screen, "Acme Okta")).getByRole("button", { name: "Edit" })).toBeDisabled();
    expect(within(row(screen, "Acme Okta")).getByRole("button", { name: "Disable" })).toBeEnabled();
    expect(within(row(screen, "Acme Okta")).getByRole("button", { name: "Delete" })).toBeEnabled();
    expect(screen.getByRole("radio", { name: /Local admins only/ })).toBeDisabled();
  });

  it("creates an LDAP connection from a preset", async () => {
    const calls = mockFetch({
      ...routes(["sso"]),
      "POST /api/v1/ee/sso/connections": { status: 201, json: LDAP },
    });
    const { screen } = renderApp("/admin/sso");
    await waitFor(() => expect(row(screen, "Acme Okta")).toBeTruthy());
    await userEvent.click(screen.getByRole("button", { name: "New connection" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByLabelText("Provider"), "ldap");
    expect(within(dialog).queryByLabelText("Client ID")).toBeNull();
    expect(within(dialog).queryByLabelText("Callback URL")).toBeNull();
    await userEvent.selectOptions(within(dialog).getByLabelText("Directory"), "openldap");
    expect(within(dialog).getByLabelText("User filter")).toHaveValue("(&(objectClass=inetOrgPerson)(uid={username}))");
    await userEvent.type(within(dialog).getByLabelText("Name"), "Corp directory");
    await userEvent.type(within(dialog).getByLabelText("Server URL"), "ldap://dc.acme.io");
    await userEvent.type(within(dialog).getByLabelText("Bind DN"), "cn=admin,dc=acme,dc=io");
    await userEvent.type(within(dialog).getByLabelText("Bind password"), "bind-pw");
    await userEvent.type(within(dialog).getByLabelText("User base DN"), "dc=acme,dc=io");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    const sent = calls.find((c) => c.method === "POST")?.body as { settings: Record<string, unknown>; secrets: object };
    expect(sent.settings).toMatchObject({ kind: "ldap", preset: "openldap", url: "ldap://dc.acme.io", username_attr: "uid" });
    expect(sent.secrets).toEqual({ bind_password: "bind-pw" });
  });

  it("warns when TLS verification is skipped", async () => {
    mockFetch(routes(["sso"]));
    const { screen } = renderApp("/admin/sso");
    await waitFor(() => expect(row(screen, "Acme Okta")).toBeTruthy());
    await userEvent.click(screen.getByRole("button", { name: "New connection" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByLabelText("Provider"), "ldap");
    await userEvent.click(within(dialog).getByLabelText("Skip TLS verification"));
    expect(within(dialog).getByRole("alert")).toHaveTextContent("not verified");
  });

  it("tests LDAP with a username, not an email", async () => {
    const calls = mockFetch({
      ...routes(["sso"]),
      "GET /api/v1/ee/sso/connections": { json: [OKTA, LDAP] },
      "POST /api/v1/ee/sso/connections/5/test": { json: { ok: true, detail: "found uid=alice", groups: ["cn=ops"] } },
    });
    const { screen } = renderApp("/admin/sso");
    await waitFor(() => expect(row(screen, "Corp directory")).toBeTruthy());
    await userEvent.click(within(row(screen, "Corp directory")).getByRole("button", { name: "Test" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByLabelText(/List groups for/)).toBeNull();
    await userEvent.type(within(dialog).getByLabelText("Look up username (optional)"), "alice");
    await userEvent.click(within(dialog).getByRole("button", { name: "Run test" }));
    await within(dialog).findByText("cn=ops");
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ username: "alice" });
  });

  it("offers LDAP only while there is none", async () => {
    mockFetch({ ...routes(["sso"]), "GET /api/v1/ee/sso/connections": { json: [OKTA, LDAP] } });
    const { screen } = renderApp("/admin/sso");
    await waitFor(() => expect(row(screen, "Corp directory")).toBeTruthy());
    await userEvent.click(screen.getByRole("button", { name: "New connection" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(within(dialog).getByLabelText("Provider")).queryByRole("option", { name: "LDAP / Active Directory" })).toBeNull();
  });
});
