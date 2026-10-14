import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

// Sub-project 3 end to end against the kind cluster from `make kind-up`:
// add it with its kubeconfig, browse, scale, read logs, open a terminal.
// Runs only with FLEETOPS_KIND=1 (make e2e-kube).

const PASSWORD = "e2e-password-1";
const ENV = `kind-${Date.now().toString(36)}`;

test.skip(!process.env.FLEETOPS_KIND, "needs a kind cluster: make e2e-kube");

test.beforeAll(async ({ request }) => {
  const status = await (await request.get("/api/v1/auth/status")).json();
  if (status.setup_required) {
    await request.post("/api/v1/auth/setup", { data: { username: "admin", password: PASSWORD } });
  }
});

test("a kubeconfig cluster can be browsed, scaled, logged and entered", async ({ page }) => {
  const kubeconfig = readFileSync(join(process.env.FLEETOPS_KUBE_IT ?? "../target/kube-it", "kubeconfig"), "utf8");

  await test.step("log in", async () => {
    await page.goto("/login");
    await page.getByLabel("Username").fill("admin");
    await page.getByLabel("Password").fill(PASSWORD);
    await page.getByRole("button", { name: "Log in" }).click();
    await expect(page.getByRole("navigation", { name: "Environment" }).or(page.getByText("No environments"))).toBeVisible();
  });

  await test.step("add the cluster", async () => {
    await page.goto("/admin/envs");
    await page.getByRole("button", { name: "Add environment" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByText("Kubernetes", { exact: true }).click();
    await dialog.getByText("Paste a kubeconfig", { exact: true }).click();
    await expect(dialog.getByRole("radio", { name: /kubeconfig/i })).toBeChecked();
    await dialog.getByLabel("Name").fill(ENV);
    await dialog.getByLabel("Kubeconfig", { exact: true }).fill(kubeconfig);
    await dialog.getByLabel("Kubeconfig", { exact: true }).blur();
    await expect(dialog.getByRole("combobox", { name: "Context" })).toHaveValue("kind-fleetops-test");
    await dialog.getByRole("button", { name: "Add" }).click();
    await expect(dialog).toBeHidden();
  });

  await test.step("browse deployments in a namespace", async () => {
    await page.goto(`/envs/${ENV}/k8s/deployments`);
    await page.getByRole("combobox", { name: "Namespace", exact: true }).click();
    await page.getByRole("option", { name: "fleetops-it", exact: true }).click();
    await expect(page.getByRole("link", { name: "web", exact: true })).toBeVisible();
  });

  await test.step("create a deployment from its template and watch kubectl change it", async () => {
    const name = `e2e-${Date.now().toString(36)}`;
    await page.goto(`/envs/${ENV}/k8s/deployments`);
    await page.getByRole("button", { name: "Create" }).click();
    const dialog = page.getByRole("dialog");
    const editor = dialog.getByRole("textbox", { name: "YAML" });
    await expect(editor).toContainText("kind: Deployment");
    await editor.click();
    await page.keyboard.press("ControlOrMeta+A");
    await page.keyboard.insertText(
      `apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: ${name}\n  namespace: fleetops-it\nspec:\n  replicas: 1\n  selector:\n    matchLabels: {app: ${name}}\n  template:\n    metadata:\n      labels: {app: ${name}}\n    spec:\n      containers:\n        - name: main\n          image: busybox:1.37\n          command: ["sh", "-c", "sleep 3600"]\n`,
    );
    await dialog.getByRole("button", { name: "Review" }).click();
    await expect(dialog.getByLabel(`Changes to Deployment ${name}`)).toBeVisible();
    await dialog.getByRole("button", { name: "Apply" }).click();
    await expect(dialog).toBeHidden();
    // From here on nothing reloads the page: the list must follow the cluster by itself.
    const row = page.getByRole("row").filter({ has: page.getByRole("link", { name, exact: true }) });
    await expect(row).toBeVisible();
    const kubeconfig = join(process.env.FLEETOPS_KUBE_IT ?? "../target/kube-it", "kubeconfig");
    execFileSync("kubectl", ["--kubeconfig", kubeconfig, "-n", "fleetops-it", "scale", `deploy/${name}`, "--replicas=2"]);
    await expect(row.getByText(/\/2$/).first()).toBeVisible({ timeout: 10_000 });
    execFileSync("kubectl", ["--kubeconfig", kubeconfig, "-n", "fleetops-it", "delete", `deploy/${name}`]);
    await expect(row).toBeHidden({ timeout: 10_000 });
  });

  await test.step("scale web to 3", async () => {
    await page.getByRole("link", { name: "web", exact: true }).click();
    await page.getByRole("button", { name: "Scale" }).click();
    await page.getByRole("dialog").getByLabel("Replicas").fill("3");
    await page.getByRole("dialog").getByRole("button", { name: "Scale" }).click();
    await expect(page.getByText(/\/3$/).first()).toBeVisible({ timeout: 60_000 });
  });

  await test.step("read the ticker's logs", async () => {
    await page.goto(`/envs/${ENV}/k8s/pods/fleetops-it/ticker?tab=logs`);
    await expect(page.getByText(/^tick \d+/).first()).toBeVisible({ timeout: 30_000 });
  });

  await test.step("open a terminal in the ticker", async () => {
    await page.goto(`/envs/${ENV}/k8s/pods/fleetops-it/ticker?tab=terminal`);
    await page.getByRole("button", { name: /connect/i }).click();
    await page.locator(".xterm").click();
    await page.keyboard.type("echo hi-from-e2e\n");
    await expect(page.locator(".xterm")).toContainText("hi-from-e2e", { timeout: 15_000 });
  });

  await test.step("read a deployment's merged logs", async () => {
    await page.goto(`/envs/${ENV}/k8s/deployments/fleetops-it/web?tab=logs`);
    await expect(page.getByRole("log").getByText("serving").first()).toBeVisible({ timeout: 30_000 });
  });

  await test.step("open a terminal from the deployment", async () => {
    await page.goto(`/envs/${ENV}/k8s/deployments/fleetops-it/web`);
    await page.getByRole("button", { name: "Terminal" }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Open terminal" }).click();
    await expect(page).toHaveURL(/\/k8s\/pods\/fleetops-it\/web-.*tab=terminal/);
    await page.getByRole("button", { name: /connect/i }).click();
    await page.locator(".xterm").click();
    await page.keyboard.type("echo hi-from-workload\n");
    await expect(page.locator(".xterm")).toContainText("hi-from-workload", { timeout: 15_000 });
  });

  await test.step("roll a Helm release back", async () => {
    const kc = join(process.env.FLEETOPS_KUBE_IT ?? "../target/kube-it", "kubeconfig");
    const chart = "../scripts/helm-it-chart";
    const helm = (...args: string[]) =>
      execFileSync(process.env.HELM ?? "helm", ["--kubeconfig", kc, "--namespace", "fleetops-it", ...args]);
    helm("uninstall", "e2e-demo", "--ignore-not-found", "--wait");
    helm("install", "e2e-demo", chart);
    helm("upgrade", "e2e-demo", chart, "--set", "extra=true");
    await page.goto(`/envs/${ENV}/k8s/helm`);
    await page.getByRole("link", { name: "e2e-demo", exact: true }).click();
    await page.getByRole("tab", { name: "History" }).click();
    await page.getByRole("button", { name: "Roll back to 1" }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText("Chart hooks are not run by FleetOps.")).toBeVisible();
    await dialog.getByRole("button", { name: "Roll back" }).click();
    await expect(dialog.getByText("Revision 3 is deployed.")).toBeVisible();
    helm("uninstall", "e2e-demo", "--wait");
  });

  await test.step("install a chart from a repository and upgrade it", async () => {
    const kc = join(process.env.FLEETOPS_KUBE_IT ?? "../target/kube-it", "kubeconfig");
    const dir = mkdtempSync(join(tmpdir(), "fo-e2e-repo-"));
    execFileSync(process.env.HELM ?? "helm", ["package", "../scripts/helm-it-chart", "-d", dir]);
    const server = createServer((req, res) => {
      try {
        res.end(readFileSync(join(dir, (req.url ?? "/").slice(1))));
      } catch {
        res.statusCode = 404;
        res.end();
      }
    });
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
    execFileSync(process.env.HELM ?? "helm", ["repo", "index", dir, "--url", url]);
    const helm = (...args: string[]) =>
      execFileSync(process.env.HELM ?? "helm", ["--kubeconfig", kc, "--namespace", "fleetops-it", ...args]);
    helm("uninstall", "e2e-installed", "--ignore-not-found", "--wait");
    await page.goto(`/envs/${ENV}/k8s/helm`);
    await page.getByRole("button", { name: "Install chart" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Release name").fill("e2e-installed");
    await dialog.getByLabel("Namespace").fill("fleetops-it");
    await dialog.getByLabel("Repository URL").fill(url);
    await dialog.getByLabel("Chart", { exact: true }).fill("demo");
    await dialog.getByRole("button", { name: "Review" }).click();
    await dialog.getByRole("button", { name: "Install" }).click();
    await expect(dialog.getByText("Revision 1 is deployed.")).toBeVisible({ timeout: 60_000 });
    await dialog.getByRole("button", { name: "Done" }).click();
    await page.goto(`/envs/${ENV}/k8s/helm/fleetops-it/e2e-installed`);
    await page.getByRole("button", { name: "Upgrade" }).click();
    await expect(page.getByRole("dialog").getByLabel("Repository URL")).toHaveValue(url);
    await page.getByRole("dialog").getByRole("button", { name: "Review" }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Upgrade" }).click();
    await expect(page.getByRole("dialog").getByText("Revision 2 is deployed.")).toBeVisible({ timeout: 60_000 });
    helm("uninstall", "e2e-installed", "--wait");
    server.close();
  });

  await test.step("an operator limited to fleetops-it sees only its pods, live", async () => {
    const user = `ns-op-${Date.now().toString(36)}`;
    await page.goto("/admin/users");
    await page.getByRole("button", { name: "Add user" }).click();
    let dialog = page.getByRole("dialog");
    await dialog.getByLabel("Username").fill(user);
    await dialog.getByLabel("Password").fill(PASSWORD);
    await dialog.getByRole("button", { name: "Add" }).click();
    await expect(dialog).toBeHidden();
    await page.getByRole("row", { name: new RegExp(user) }).getByRole("button", { name: "Access" }).click();
    dialog = page.getByRole("dialog");
    await dialog.getByLabel(`Role on ${ENV}`).selectOption("operator");
    await dialog.getByText("Entire cluster", { exact: true }).click();
    await dialog.getByLabel(`Namespaces on ${ENV}`).fill("fleetops-it");
    await dialog.getByLabel(`Namespaces on ${ENV}`).press("Enter");
    await dialog.getByRole("button", { name: "Save" }).click();
    await expect(dialog).toBeHidden();

    const ctx = await page.context().browser()!.newContext();
    const op = await ctx.newPage();
    await op.goto("/login");
    await op.getByLabel("Username").fill(user);
    await op.getByLabel("Password").fill(PASSWORD);
    await op.getByRole("button", { name: "Log in" }).click();
    await expect(op.getByRole("navigation", { name: "Environment" })).toBeVisible();
    await op.goto(`/envs/${ENV}/k8s/pods`);
    await op.getByRole("combobox", { name: "Namespace", exact: true }).click();
    await op.getByRole("option", { name: "All namespaces" }).click();
    await expect(op.getByRole("link", { name: "ticker", exact: true })).toBeVisible();
    // The picker offers only the namespaces in the grant.
    await op.getByRole("combobox", { name: "Namespace", exact: true }).click();
    await expect(op.getByRole("option")).toHaveText([/^All namespaces/, "fleetops-it"]);
    await op.keyboard.press("Escape");
    const outside = `e2e-out-${Date.now().toString(36)}`;
    const inside = `e2e-in-${Date.now().toString(36)}`;
    const kc = join(process.env.FLEETOPS_KUBE_IT ?? "../target/kube-it", "kubeconfig");
    execFileSync("kubectl", ["--kubeconfig", kc, "-n", "default", "run", outside, "--image=busybox:1.37", "--", "sleep", "300"]);
    execFileSync("kubectl", ["--kubeconfig", kc, "-n", "fleetops-it", "run", inside, "--image=busybox:1.37", "--", "sleep", "300"]);
    try {
      await expect(op.getByRole("link", { name: inside, exact: true })).toBeVisible({ timeout: 30_000 });
      await expect(op.getByRole("link", { name: outside, exact: true })).toHaveCount(0);
    } finally {
      execFileSync("kubectl", ["--kubeconfig", kc, "-n", "default", "delete", "pod", outside, "--wait=false"]);
      execFileSync("kubectl", ["--kubeconfig", kc, "-n", "fleetops-it", "delete", "pod", inside, "--wait=false"]);
      await ctx.close();
    }
  });

  await test.step("browse to a custom resource and edit it", async () => {
    const kc = join(process.env.FLEETOPS_KUBE_IT ?? "../target/kube-it", "kubeconfig");
    const name = `e2e-w-${Date.now().toString(36)}`;
    execFileSync("kubectl", ["--kubeconfig", kc, "-n", "fleetops-it", "apply", "-f", "-"], {
      input: `apiVersion: example.fleetops.io/v1\nkind: Widget\nmetadata: {name: ${name}}\nspec: {size: 2}\n`,
    });
    try {
      await page.goto(`/envs/${ENV}`);
      const nav = page.getByRole("navigation", { name: "Environment" });
      await nav.getByRole("button", { name: /Custom Resources/ }).click();
      await nav.getByRole("button", { name: /example\.fleetops\.io/ }).click();
      await nav.getByRole("link", { name: "Widget", exact: true }).click();
      const row = page.getByRole("row").filter({ has: page.getByRole("link", { name, exact: true }) });
      await expect(row).toContainText("2");
      await row.getByRole("link", { name, exact: true }).click();
      await page.getByRole("button", { name: "Edit YAML" }).click();
      const dialog = page.getByRole("dialog");
      const editor = dialog.getByRole("textbox", { name: "YAML" });
      await expect(editor).toContainText("size: 2");
      const yaml = await editor.innerText();
      await editor.click();
      await page.keyboard.press("ControlOrMeta+A");
      await page.keyboard.insertText(yaml.replace("size: 2", "size: 9"));
      await dialog.getByRole("button", { name: "Review" }).click();
      await dialog.getByRole("button", { name: "Apply" }).click();
      await expect(dialog).toBeHidden();
      await page.getByRole("navigation", { name: "Environment" }).getByRole("link", { name: "Widget", exact: true }).click();
      await expect(page.getByRole("row").filter({ has: page.getByRole("link", { name, exact: true }) })).toContainText("9");
    } finally {
      execFileSync("kubectl", ["--kubeconfig", kc, "-n", "fleetops-it", "delete", "widget", name, "--wait=false"]);
    }
  });

  await test.step("scale back and remove the environment", async () => {
    await page.goto(`/envs/${ENV}/k8s/deployments/fleetops-it/web`);
    await page.getByRole("button", { name: "Scale" }).click();
    await page.getByRole("dialog").getByLabel("Replicas").fill("2");
    await page.getByRole("dialog").getByRole("button", { name: "Scale" }).click();
    await page.goto("/admin/envs");
    const row = page.getByRole("row", { name: new RegExp(ENV) });
    await row.getByRole("button", { name: "Delete" }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Delete" }).click();
    await expect(row).toBeHidden();
  });
});
