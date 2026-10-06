import { useEffect, useState } from "react";

const EVENT = "fleetops:apply-yaml";

/** Opens the Apply YAML dialog for `env` from anywhere (command palette, top bar). */
export function openApplyYaml(env: string) {
  window.dispatchEvent(new CustomEvent(EVENT, { detail: env }));
}

/** Whether the Apply YAML dialog was asked for on `env`; the setter closes it. */
export function useApplyYamlRequests(env: string | undefined): [boolean, (open: boolean) => void] {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const on = (e: Event) => {
      if ((e as CustomEvent<string>).detail === env) setOpen(true);
    };
    window.addEventListener(EVENT, on);
    return () => window.removeEventListener(EVENT, on);
  }, [env]);
  return [open, setOpen];
}
