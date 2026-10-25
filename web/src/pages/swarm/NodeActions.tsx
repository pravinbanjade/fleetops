import { useQueryClient } from "@tanstack/react-query";
import { ArrowDownToLine, ArrowUpToLine, CirclePause, CirclePlay, LogOut, Tags, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { useEnvAccess } from "@/api/access";
import { removeSwarmNode, updateSwarmNode } from "@/api/gen/sdk.gen";
import type { NodeChange } from "@/api/gen/types.gen";
import { swarmKeys, swarmNodeQuery } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { RowMenu, type QuickItem } from "@/components/RowMenu";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { errorMessage } from "@/lib/errors";
import { useGuarded } from "./guarded";

export interface NodeRef {
  id: string;
  hostname: string;
  role?: string | null;
  state?: string | null;
  availability?: string | null;
  labels?: Record<string, string>;
}

export const DRAIN_WARNING =
  "Draining stops every task on this node, including its FleetOps agent; its node pages go offline until it is active again.";

/** `key=value` per line; blank lines are skipped. */
export function parseLabels(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  text.split("\n").forEach((line, i) => {
    const l = line.trim();
    if (!l) return;
    const eq = l.indexOf("=");
    if (eq <= 0) throw new Error(`line ${i + 1}: use key=value`);
    out[l.slice(0, eq).trim()] = l.slice(eq + 1).trim();
  });
  return out;
}

type Asking = "drain" | "remove" | "labels" | null;

/** A node's ⋮ menu: availability, role, labels and removal. */
export function NodeActions({ env, node }: { env: string; node: NodeRef }) {
  const qc = useQueryClient();
  const { can } = useEnvAccess(env);
  const { run, dialog } = useGuarded();
  const [asking, setAsking] = useState<Asking>(null);
  const [labels, setLabels] = useState("");
  const name = node.hostname || node.id;

  const change = (body: NodeChange, done: string) =>
    run((confirm) => updateSwarmNode({ path: { env, id: node.id }, body: { ...body, confirm }, throwOnError: true }), name)
      .then(async (ran) => {
        if (!ran) return;
        toast.success(done);
        await qc.invalidateQueries({ queryKey: swarmKeys.all(env) });
      })
      .catch((e) => toast.error(errorMessage(e)));

  const remove = () =>
    run((confirm) => removeSwarmNode({ path: { env, id: node.id }, query: { confirm }, throwOnError: true }), name)
      .then(async (ran) => {
        if (!ran) return;
        toast.success(`${name} removed`);
        await qc.invalidateQueries({ queryKey: swarmKeys.all(env) });
      })
      .catch((e) => toast.error(errorMessage(e)));

  const items: QuickItem[] = [];
  if (can("swarm-nodes.operate")) {
    if (node.availability !== "active")
      items.push({ id: "active", label: "Activate", icon: CirclePlay, onSelect: () => void change({ availability: "active" }, `${name} is active`) });
    if (node.availability !== "pause")
      items.push({ id: "pause", label: "Pause", icon: CirclePause, onSelect: () => void change({ availability: "pause" }, `${name} is paused`) });
    if (node.availability !== "drain") items.push({ id: "drain", label: "Drain", icon: LogOut, onSelect: () => setAsking("drain") });
    if (node.role === "worker")
      items.push({ id: "promote", label: "Promote to manager", icon: ArrowUpToLine, onSelect: () => void change({ role: "manager" }, `${name} is a manager`) });
    if (node.role === "manager")
      items.push({ id: "demote", label: "Demote to worker", icon: ArrowDownToLine, onSelect: () => void change({ role: "worker" }, `${name} is a worker`) });
    items.push({
      id: "labels",
      label: "Edit labels",
      icon: Tags,
      onSelect: () => {
        // Saving replaces every label, so start from the node's current ones,
        // fetched fresh: a list row may not carry them.
        void qc
          .fetchQuery({ ...swarmNodeQuery(env, node.id), staleTime: 0 })
          .then((n) => {
            setLabels(Object.entries(n?.labels ?? {}).map(([k, v]) => `${k}=${v}`).join("\n"));
            setAsking("labels");
          })
          .catch((e) => toast.error(errorMessage(e)));
      },
    });
  }
  if (can("swarm-nodes.delete"))
    items.push({ id: "remove", label: node.state === "ready" ? "Remove (only a node that is down)" : "Remove", icon: Trash2, danger: true, disabled: node.state === "ready", onSelect: () => setAsking("remove") });

  const close = (open: boolean) => !open && setAsking(null);
  return (
    <>
      <RowMenu label={name} items={items} />
      {asking === "drain" && (
        <ConfirmDialog open onOpenChange={close} title={`Drain ${name}?`} description={DRAIN_WARNING} confirmLabel="Drain" onConfirm={async () => void change({ availability: "drain" }, `${name} is draining`)} />
      )}
      {asking === "remove" && (
        <ConfirmDialog open onOpenChange={close} title={`Remove ${name}?`} description="The swarm forgets this node. To bring it back, join it again." confirmLabel="Remove" onConfirm={async () => void remove()} />
      )}
      {asking === "labels" && (
        <ConfirmDialog
          open
          onOpenChange={close}
          title={`Labels on ${name}`}
          description="One key=value per line. These replace every label on the node."
          confirmLabel="Save labels"
          destructive={false}
          onConfirm={async () => {
            const parsed = parseLabels(labels);
            void change({ labels: parsed }, `Labels saved on ${name}`);
          }}
        >
          <div className="grid gap-1.5">
            <Label htmlFor="node-labels">Labels</Label>
            <Textarea id="node-labels" className="font-mono text-xs" rows={6} value={labels} onChange={(e) => setLabels(e.target.value)} />
          </div>
        </ConfirmDialog>
      )}
      {dialog}
    </>
  );
}
