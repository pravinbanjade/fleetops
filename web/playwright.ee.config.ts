import { defineConfig } from "@playwright/test";
import base from "./playwright.config";

// The Enterprise suite (`make e2e-ee`): a release build with the
// ee-test-license-key feature, so the committed test license installs.
export default defineConfig({ ...base, testDir: "../ee/web/e2e" });
