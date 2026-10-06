import { lazy, Suspense } from "react";
import { useApplyYamlRequests } from "./applyYaml";

const YamlDialog = lazy(() => import("./YamlDialog"));

/** The namespace the user last picked on `env`, for documents that name none. */
function lastNamespace(env: string) {
  try {
    return localStorage.getItem(`fleetops.ns.${env}`) || "default";
  } catch {
    return "default";
  }
}

/** Shows the Apply YAML dialog when something asks for it on `env`. */
export function ApplyYamlHost({ env }: { env: string }) {
  const [open, setOpen] = useApplyYamlRequests(env);
  if (!open) return null;
  return (
    <Suspense fallback={null}>
      <YamlDialog env={env} target={{ mode: "apply", namespace: lastNamespace(env) }} onClose={() => setOpen(false)} />
    </Suspense>
  );
}
