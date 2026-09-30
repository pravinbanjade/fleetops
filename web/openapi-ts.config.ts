import { defineConfig } from "@hey-api/openapi-ts";

// `pnpm gen:api` regenerates src/api/gen from the committed server document;
// `pnpm check:api` fails when the committed client has drifted from it.
export default defineConfig({
  input: "../crates/fleetops-api/openapi.json",
  output: "src/api/gen",
  plugins: [{ name: "@hey-api/client-fetch", throwOnError: true }, "@hey-api/typescript", "@hey-api/sdk"],
});
