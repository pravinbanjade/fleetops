import { useQuery } from "@tanstack/react-query";
import { useCallback, useState } from "react";
import { kubeListQuery } from "@/api/queries";

const storageKey = (env: string) => `fleetops.ns.${env}`;

function read(env: string): string | undefined {
  try {
    return localStorage.getItem(storageKey(env)) || undefined;
  } catch {
    return undefined;
  }
}

/** The namespace a Kubernetes environment's lists show, remembered per environment. `undefined` is all. */
export function useNamespace(env: string): [string | undefined, (ns?: string) => void] {
  const [ns, setNs] = useState(() => read(env));
  const set = useCallback(
    (next?: string) => {
      setNs(next);
      try {
        if (next) localStorage.setItem(storageKey(env), next);
        else localStorage.removeItem(storageKey(env));
      } catch {
        /* storage blocked: the choice lasts this page only */
      }
    },
    [env],
  );
  return [ns, set];
}

export function NamespacePicker({ env, value, onChange }: { env: string; value?: string; onChange: (ns?: string) => void }) {
  const { data, isError } = useQuery(kubeListQuery(env, "namespaces"));
  const [typed, setTyped] = useState(value ?? "");
  // Credentials scoped to one namespace often may not list namespaces: let
  // the user name the one they can read instead.
  if (isError) {
    return (
      <form
        className="flex items-center gap-2 text-sm"
        onSubmit={(e) => {
          e.preventDefault();
          onChange(typed.trim() || undefined);
        }}
      >
        <label htmlFor={`ns-${env}`} className="text-muted-foreground">
          Namespace
        </label>
        <input
          id={`ns-${env}`}
          aria-label="Namespace"
          placeholder="All namespaces"
          className="h-8 w-40 rounded-md border bg-background px-2"
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          onBlur={() => onChange(typed.trim() || undefined)}
        />
      </form>
    );
  }
  const names = (data?.rows ?? []).map((r) => r.name);
  return (
    <label className="flex items-center gap-2 text-sm">
      <span className="text-muted-foreground">Namespace</span>
      <select
        aria-label="Namespace"
        className="h-8 rounded-md border bg-background px-2"
        value={value ?? ""}
        onChange={(e) => onChange(e.target.value || undefined)}
      >
        <option value="">All namespaces</option>
        {names.map((n) => (
          <option key={n} value={n}>
            {n}
          </option>
        ))}
      </select>
    </label>
  );
}
