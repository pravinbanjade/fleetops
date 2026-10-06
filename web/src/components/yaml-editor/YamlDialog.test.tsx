import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { configureApi } from "@/api/client";
import { mockFetch } from "@/test/fetch";
import YamlDialog from "./YamlDialog";

vi.mock("./CodeMirrorField", () => ({
  default: ({ value, onChange, label }: { value: string; onChange: (v: string) => void; label: string }) => (
    <textarea aria-label={label} value={value} onChange={(e) => onChange(e.target.value)} />
  ),
}));

const wrap = (ui: ReactNode) => render(<QueryClientProvider client={new QueryClient()}>{ui}</QueryClientProvider>);
const result = (over = {}) => ({
  index: 0,
  api_version: "apps/v1",
  kind: "Deployment",
  namespace: "apps",
  name: "web",
  action: "configured",
  before: "replicas: 1\n",
  after: "replicas: 2\n",
  error: null,
  ...over,
});

describe("YamlDialog", () => {
  beforeEach(() => configureApi());

  it("edits: review shows the dry-run diff, apply writes", async () => {
    const calls = mockFetch({
      "GET /api/v1/envs/k/k8s/deployments/apps/web/edit": { json: { yaml: "replicas: 1\n", resource_version: "5" } },
      "PUT /api/v1/envs/k/k8s/deployments/apps/web": { json: result() },
    });
    const onClose = vi.fn();
    wrap(<YamlDialog env="k" target={{ mode: "edit", kind: "deployments", ns: "apps", name: "web" }} onClose={onClose} />);
    const editor = await screen.findByRole("textbox", { name: "YAML" });
    await waitFor(() => expect(editor).toHaveValue("replicas: 1\n"));
    await userEvent.clear(editor);
    await userEvent.type(editor, "replicas: 2\n");
    await userEvent.click(screen.getByRole("button", { name: "Review" }));
    const diff = await screen.findByLabelText("Changes to Deployment web");
    expect(diff).toHaveTextContent("- replicas: 1");
    expect(diff).toHaveTextContent("+ replicas: 2");
    expect(calls.find((c) => c.method === "PUT")!.body).toMatchObject({ resource_version: "5", dry_run: true });
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(calls.filter((c) => c.method === "PUT").at(-1)!.body).toMatchObject({ dry_run: false });
  });

  it("refuses to review YAML that does not parse", async () => {
    const calls = mockFetch({
      "GET /api/v1/envs/k/k8s/templates": { json: { configmaps: "kind: ConfigMap\nmetadata:\n  namespace: NAMESPACE\n" } },
    });
    wrap(<YamlDialog env="k" target={{ mode: "create", kind: "configmaps", namespace: "apps" }} onClose={() => {}} />);
    const editor = await screen.findByRole("textbox", { name: "YAML" });
    await waitFor(() => expect(editor).toHaveValue("kind: ConfigMap\nmetadata:\n  namespace: apps\n"));
    await userEvent.type(editor, "bad: [[");
    await userEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/line/);
    expect(calls.some((c) => c.method === "POST")).toBe(false);
  });

  it("shows the live YAML on a conflict and can reapply on top of it", async () => {
    let opened = 0;
    mockFetch({
      "GET /api/v1/envs/k/k8s/deployments/apps/web/edit": () =>
        opened++ === 0 ? { json: { yaml: "replicas: 1\n", resource_version: "5" } } : { json: { yaml: "replicas: 3\n", resource_version: "9" } },
      "PUT /api/v1/envs/k/k8s/deployments/apps/web": (c) =>
        (c.body as { resource_version: string }).resource_version === "5"
          ? { status: 409, json: { code: "conflict", message: "changed since you opened it", request_id: null } }
          : { json: result() },
    });
    wrap(<YamlDialog env="k" target={{ mode: "edit", kind: "deployments", ns: "apps", name: "web" }} onClose={() => {}} />);
    const editor = await screen.findByRole("textbox", { name: "YAML" });
    await waitFor(() => expect(editor).toHaveValue("replicas: 1\n"));
    await userEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(await screen.findByText(/changed since you opened it/)).toBeInTheDocument();
    expect(screen.getByLabelText("Live YAML")).toHaveTextContent("replicas: 3");
    await userEvent.click(screen.getByRole("button", { name: "Reload and reapply my edit" }));
    expect(await screen.findByLabelText("Changes to Deployment web")).toBeInTheDocument();
  });

  it("applies several documents, lists failures, and offers force on a conflict", async () => {
    const calls = mockFetch({
      "POST /api/v1/envs/k/k8s/apply": (c) => ({
        json: (c.body as { force: boolean }).force
          ? [result({ action: "configured" })]
          : [result({ action: null, after: null, error: { code: "conflict", message: 'conflict with "kubectl": .spec.replicas' } })],
      }),
    });
    wrap(<YamlDialog env="k" target={{ mode: "apply", namespace: "apps" }} onClose={() => {}} />);
    await userEvent.type(await screen.findByRole("textbox", { name: "YAML" }), "kind: Deployment\n");
    await userEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(await screen.findByText(/conflict with "kubectl"/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Force apply" }));
    await userEvent.click(await screen.findByRole("button", { name: "Force apply anyway" }));
    await waitFor(() => expect(calls.at(-1)!.body).toMatchObject({ force: true, dry_run: false }));
  });
});
