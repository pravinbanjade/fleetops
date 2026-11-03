import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Save } from "lucide-react";
import type { EnvSettingsView } from "@/api/gen/types.gen";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { toastError } from "@/lib/errors";
import { MirrorFields, useMirrorDraft } from "./MirrorFields";
import { putEnvSettings, scannerLabel, settingsQuery } from "./security";
import { choiceOf, enabledOf, scannerTone, type Choice } from "./SecuritySettingsPage";

const docker = (kind: string) => kind === "docker-local" || kind === "docker-agent" || kind === "swarm-agent";

/** In an environment's panel: its own scanning setting, mirror and engine socket. */
export function EnvSecuritySettings({ env, kind }: { env: string; kind: string }) {
  const s = useQuery(settingsQuery).data;
  const e = s?.envs.find((v) => v.env === env);
  if (!s || !e) return null;
  return <Form key={env} e={e} kind={kind} defaultImage={s.global.mirror?.trivy_image ?? s.global.default_trivy_image} />;
}

function Form({ e, kind, defaultImage }: { e: EnvSettingsView; kind: string; defaultImage: string }) {
  const qc = useQueryClient();
  const [choice, setChoice] = useState<Choice>(choiceOf(e.enabled));
  const [own, setOwn] = useState(!!e.mirror);
  const [socket, setSocket] = useState(e.socket_path ?? "");
  const mirror = useMirrorDraft(e.mirror);
  const save = useMutation({
    mutationFn: () =>
      putEnvSettings(e.env, {
        enabled: enabledOf(choice),
        mirror: own ? mirror.body() : null,
        socket_path: docker(kind) ? socket.trim() || null : null,
      }),
    onSuccess: (next) => qc.setQueryData(settingsQuery.queryKey, next),
    onError: toastError,
  });
  const id = `env-scan-${e.env}`;
  return (
    <section aria-label="Security scanning settings" className="grid gap-3 rounded-md bg-raised p-3">
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor={`${id}-choice`} className="text-sm font-medium">
          Security scanning
        </Label>
        <StateBadge state={scannerLabel(e.status)} tone={scannerTone(e.status)} />
      </div>
      <NativeSelect id={`${id}-choice`} value={choice} onChange={(ev) => setChoice(ev.target.value as Choice)}>
        <option value="inherit">Inherit the global setting</option>
        <option value="on">On</option>
        <option value="off">Off</option>
      </NativeSelect>
      <label className="flex items-center gap-2 text-sm">
        <Checkbox checked={own} onCheckedChange={(v) => setOwn(v === true)} />
        Use its own mirror
      </label>
      {own && <MirrorFields draft={mirror} idPrefix={`${id}-mirror`} defaultImage={defaultImage} />}
      {docker(kind) && (
        <div className="grid gap-1.5">
          <Label htmlFor={`${id}-socket`}>Engine socket</Label>
          <Input id={`${id}-socket`} className="font-ident" value={socket} placeholder="Automatic" onChange={(ev) => setSocket(ev.target.value)} />
          <p className="text-xs text-meta">The engine socket's path on the host, mounted into the scan container. Set it for rootless engines.</p>
        </div>
      )}
      <Button size="sm" className="justify-self-start" disabled={save.isPending} onClick={() => save.mutate()}>
        <Save aria-hidden />
        Save
      </Button>
    </section>
  );
}
