import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { parse } from "yaml";
import { describe, expect, it } from "vitest";
import { kubeRoutes, loggedIn, renderApp } from "@/test/app";
import { mockFetch } from "@/test/fetch";
import { TWEAKS } from "./model";

const yaml = (env: string, annotations = "") => `apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  namespace: apps
${annotations}spec:
  template:
    spec:
      containers:
        - name: main
          image: web:1
          env:
${env}`;
const live = yaml("            - name: LOG_LEVEL\n              value: info\n            - name: DB\n              valueFrom:\n                secretKeyRef: { name: db, key: url }\n");
const deployment = (text: string) => ({
  row: { name: "web", namespace: "apps", created: 1, status: "1/1", health: "ok", columns: { ready: "1/1" } },
  yaml: text,
  conditions: [],
  owners: [],
  containers: ["main"],
});
const result = { index: 0, kind: "Deployment", api_version: "apps/v1", name: "web", namespace: "apps", action: "configured", before: "a\n", after: "b\n" };

describe("tweaking a workload's environment", () => {
  it("edits a value, reviews it as a dry run with what it replaces, then applies it", async () => {
    const calls = mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/k8s/deployments/apps/web": { json: deployment(live) },
      "GET /api/v1/envs/k/k8s/deployments/apps/web/edit": { json: { yaml: live, resource_version: "7" } },
      "PUT /api/v1/envs/k/k8s/deployments/apps/web": { json: result },
    });
    renderApp("/envs/k/k8s/deployments/apps/web");
    const env = await screen.findByRole("region", { name: "Environment" });
    expect(within(env).getByText("info")).toBeInTheDocument();
    expect(within(env).getByRole("link", { name: "Secret db" })).toBeInTheDocument();

    await userEvent.click(within(env).getByRole("button", { name: "Edit env" }));
    const value = within(env).getByLabelText("Value of LOG_LEVEL");
    await userEvent.clear(value);
    await userEvent.type(value, "debug");
    expect(within(env).getByText("1 change")).toBeInTheDocument();
    await userEvent.click(within(env).getByRole("button", { name: "Review" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/starts a rollout/)).toBeInTheDocument();
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Apply" })).toBeEnabled());
    const dry = calls.find((c) => c.method === "PUT")!.body as { yaml: string; resource_version: string; dry_run: boolean };
    expect(dry).toMatchObject({ resource_version: "7", dry_run: true });
    const sent = parse(dry.yaml);
    expect(sent.spec.template.spec.containers[0].env[0]).toEqual({ name: "LOG_LEVEL", value: "debug" });
    expect(JSON.parse(sent.metadata.annotations[TWEAKS]).env).toEqual({ main: { LOG_LEVEL: { value: "info" } } });

    await userEvent.click(within(dialog).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(calls.filter((c) => c.method === "PUT").at(-1)!.body).toMatchObject({ dry_run: false, yaml: dry.yaml }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("shows what a tweak replaced and reverts it through the same review", async () => {
    const record = JSON.stringify({ by: "ana", at: 1, env: { main: { LOG_LEVEL: { value: "info" } } } });
    const tweaked = yaml("            - name: LOG_LEVEL\n              value: debug\n", `  annotations:\n    ${TWEAKS}: '${record}'\n`);
    const calls = mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/k8s/deployments/apps/web": { json: deployment(tweaked) },
      "GET /api/v1/envs/k/k8s/deployments/apps/web/edit": { json: { yaml: tweaked, resource_version: "8" } },
      "PUT /api/v1/envs/k/k8s/deployments/apps/web": { json: result },
    });
    renderApp("/envs/k/k8s/deployments/apps/web");
    const env = await screen.findByRole("region", { name: "Environment" });
    expect(within(env).getByText(/Changed by ana/)).toBeInTheDocument();
    expect(within(env).getByText("info")).toBeInTheDocument();
    await userEvent.click(within(env).getByRole("button", { name: "Revert LOG_LEVEL" }));
    const dialog = await screen.findByRole("dialog", { name: "Revert web?" });
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Revert" })).toBeEnabled());
    const sent = parse((calls.find((c) => c.method === "PUT")!.body as { yaml: string }).yaml);
    expect(sent.spec.template.spec.containers[0].env).toEqual([{ name: "LOG_LEVEL", value: "info" }]);
    expect(sent.metadata.annotations?.[TWEAKS]).toBeUndefined();
  });

  it("keeps a pod's environment read-only and points to its workload", async () => {
    const pod = {
      row: { name: "web-1", namespace: "apps", created: 1, status: "Running", health: "ok", columns: {} },
      yaml: "kind: Pod\nspec:\n  containers:\n    - name: main\n      image: web:1\n      env:\n        - name: A\n          value: b\n",
      conditions: [],
      owners: [{ kind: "ReplicaSet", name: "web-7d9", link: "replicasets" }, { kind: "Deployment", name: "web", link: "deployments" }],
      containers: ["main"],
    };
    mockFetch({ ...loggedIn, ...kubeRoutes, "GET /api/v1/envs/k/k8s/pods/apps/web-1": { json: pod } });
    renderApp("/envs/k/k8s/pods/apps/web-1");
    const env = await screen.findByRole("region", { name: "Environment" });
    expect(within(env).queryByRole("button", { name: "Edit env" })).not.toBeInTheDocument();
    expect(within(env).getByRole("link", { name: "Deployment web" })).toBeInTheDocument();
  });

  it("refuses a new variable with a bad or taken name", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, "GET /api/v1/envs/k/k8s/deployments/apps/web": { json: deployment(live) } });
    renderApp("/envs/k/k8s/deployments/apps/web");
    const env = await screen.findByRole("region", { name: "Environment" });
    await userEvent.click(within(env).getByRole("button", { name: "Edit env" }));
    await userEvent.click(within(env).getByRole("button", { name: /Add variable/ }));
    await userEvent.type(within(env).getByLabelText("New variable name"), "LOG_LEVEL");
    expect(within(env).getByText("LOG_LEVEL is already set here")).toBeInTheDocument();
    expect(within(env).getByRole("button", { name: "Review" })).toBeDisabled();
  });
});
