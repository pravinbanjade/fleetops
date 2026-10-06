import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
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
    await page.getByRole("combobox", { name: "Namespace" }).selectOption("fleetops-it");
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
