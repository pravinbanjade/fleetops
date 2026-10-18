import { defineConfig } from "@playwright/test";
import base from "./playwright.config";

// The Enterprise suite (`make e2e-ee`): a release build with the
// ee-test-license-key feature, so the committed test license installs.
// A mock identity provider runs beside it for single sign-on.
const idpPort = Number(process.env.MOCK_IDP_PORT ?? 9124);
const server = Array.isArray(base.webServer) ? base.webServer[0] : base.webServer!;

export default defineConfig({
  ...base,
  testDir: "../ee/web/e2e",
  webServer: [
    {
      command: `exec node ../ee/web/e2e/mock-idp.mjs`,
      url: `http://127.0.0.1:${idpPort}/health`,
      env: { MOCK_IDP_PORT: String(idpPort) },
      reuseExistingServer: false,
      timeout: 10_000,
    },
    { ...server, env: { ...process.env, FLEETOPS_PUBLIC_URL: String(base.use?.baseURL) } as Record<string, string> },
  ],
});
