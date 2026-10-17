import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ENVS, KUBE_ENV, kubeRoutes, loggedIn, renderApp } from "@/test/app";
import { mockFetch } from "@/test/fetch";

/** An environment the caller holds one custom role on, everywhere. */
const holding = <T extends object>(e: T, permissions: string[]) => ({
  ...e,
  access: { role: "operator", role_name: "Deployer", namespaces: null, grants: [{ namespaces: null, permissions }] },
});

const DEPLOYER = ["containers.view", "containers.create", "containers.operate", "volumes.view", "volumes.create"];

describe("a custom role", () => {
  it("offers Create but not Prune on containers", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs": { json: ENVS.map((e) => holding(e, DEPLOYER)) } });
    renderApp("/envs/local/containers");
    await screen.findByText("No containers in this environment.");
    expect(await screen.findByRole("link", { name: /Create container/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Prune/ })).toBeNull();
  });

  it("offers Create volume but not Prune on volumes", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs": { json: ENVS.map((e) => holding(e, DEPLOYER)) } });
    renderApp("/envs/local/volumes");
    expect(await screen.findByRole("button", { name: "Create volume" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Prune" })).toBeNull();
  });

  it("shows the role's name in the environment switcher", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs": { json: ENVS.map((e) => holding(e, DEPLOYER)) } });
    renderApp("/envs/local/containers");
    await screen.findByRole("heading", { name: "Containers" });
    await userEvent.click(screen.getAllByRole("button", { name: /local/ })[0]);
    expect(await screen.findByText("Deployer")).toBeInTheDocument();
  });
});

const secret = {
  row: { name: "creds", namespace: "apps", created: 1, status: "Opaque", health: "ok", columns: {} },
  yaml: "kind: Secret\ndata:\n  password: ••••\n",
  conditions: [],
  owners: [],
  containers: [],
};

describe("secret values", () => {
  const routes = (permissions: string[]) => ({
    ...loggedIn,
    ...kubeRoutes,
    "GET /api/v1/envs": { json: [...ENVS, holding(KUBE_ENV, permissions)] },
    "GET /api/v1/envs/k/k8s/secrets/apps/creds": { json: secret },
    "GET /api/v1/envs/k/k8s/secrets/apps/creds/values": { json: [{ key: "password", value: "s3cret", binary: false }] },
  });

  it("are offered only with Reveal, and shown on request", async () => {
    mockFetch(routes(["secrets.view", "secrets.reveal"]));
    renderApp("/envs/k/k8s/secrets/apps/creds");
    await userEvent.click(await screen.findByRole("button", { name: "Show values" }));
    const values = await screen.findByRole("region", { name: "Secret values" });
    expect(values).toHaveTextContent("password");
    expect(values).toHaveTextContent("s3cret");
  });

  it("are not offered without Reveal", async () => {
    mockFetch(routes(["secrets.view"]));
    renderApp("/envs/k/k8s/secrets/apps/creds");
    await screen.findByRole("heading", { name: "creds" });
    await waitFor(() => expect(screen.queryByRole("button", { name: "Show values" })).toBeNull());
  });
});
