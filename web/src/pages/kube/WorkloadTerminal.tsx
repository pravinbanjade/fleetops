import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { TerminalSquare } from "lucide-react";
import { useState } from "react";
import type { KubeKind, KubeRow } from "@/api/gen/types.gen";
import { workloadPodsQuery } from "@/api/queries";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { errorMessage } from "@/lib/errors";
import { parseContainers } from "./ContainerSquares";

/** Ready pods first, newest first within each. */
export function sortPods(rows: KubeRow[]): KubeRow[] {
  return [...rows].sort((a, b) => Number(b.health === "ok") - Number(a.health === "ok") || b.created - a.created);
}

/** A shell in one of a workload's pods: pick the pod and container, then the pod's Terminal tab (spec 3i §6). */
export function WorkloadTerminal({ env, kind, ns, name }: { env: string; kind: KubeKind; ns: string; name: string }) {
  const [open, setOpen] = useState(false);
  const pods = useQuery({ ...workloadPodsQuery(env, kind, ns, name), enabled: open });
  const navigate = useNavigate();
  const rows = sortPods(pods.data?.rows ?? []);
  const [pod, setPod] = useState<string>();
  const chosen = rows.find((r) => r.name === pod) ?? rows[0];
  const containers = parseContainers(chosen?.columns.containers).filter((c) => !c.init);
  const [container, setContainer] = useState<string>();
  const c = containers.find((x) => x.name === container)?.name ?? containers[0]?.name;
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button variant="outline" size="sm">
          <TerminalSquare aria-hidden /> Terminal
        </Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Open a terminal in {name}</DialogTitle>
        </DialogHeader>
        {pods.error ? (
          <p role="alert" className="text-sm text-destructive">
            {errorMessage(pods.error)}
          </p>
        ) : pods.isPending ? (
          <p className="text-sm text-muted-foreground">Loading pods…</p>
        ) : rows.length === 0 ? (
          <p className="text-sm text-muted-foreground">This workload has no pods.</p>
        ) : (
          <div className="grid gap-3">
            <div className="grid gap-1.5">
              <Label htmlFor="wt-pod">Pod</Label>
              <NativeSelect
                id="wt-pod"
                value={chosen?.name}
                onChange={(e) => {
                  setPod(e.target.value);
                  setContainer(undefined);
                }}
              >
                {rows.map((r) => (
                  <option key={r.name} value={r.name}>
                    {r.name} · {r.status}
                  </option>
                ))}
              </NativeSelect>
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="wt-container">Container</Label>
              <NativeSelect id="wt-container" value={c} onChange={(e) => setContainer(e.target.value)}>
                {containers.map((x) => (
                  <option key={x.name} value={x.name}>
                    {x.name}
                  </option>
                ))}
              </NativeSelect>
            </div>
          </div>
        )}
        <DialogFooter>
          <Button
            disabled={!chosen || !c}
            onClick={() =>
              void navigate({
                to: "/envs/$env/k8s/$kind/$ns/$name",
                params: { env, kind: "pods", ns, name: chosen!.name },
                search: { tab: "terminal", container: c },
              })
            }
          >
            Open terminal
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
