import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { HardDriveDownload, Power, PowerOff, Save, ServerCog, ShieldAlert } from "lucide-react";
import type { EnvSettingsView, SecuritySettings } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Page } from "@/components/Page";
import { Panel } from "@/components/Panel";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { NativeSelect } from "@/components/ui/native-select";
import { toastError } from "@/lib/errors";
import { keepMirror, MirrorFields, useMirrorDraft } from "./MirrorFields";
import { putEnvSettings, putSettings, scannerLabel, securityKeys, settingsQuery, useSecurityLive } from "./security";

export function scannerTone(s: EnvSettingsView["status"]): "run" | "wait" | "fail" | "idle" {
  return s.state === "ready" ? "run" : s.state === "preparing" ? "wait" : s.state === "unavailable" ? "fail" : "idle";
}

export type Choice = "inherit" | "on" | "off";
export const choiceOf = (enabled: boolean | null | undefined): Choice => (enabled == null ? "inherit" : enabled ? "on" : "off");
export const enabledOf = (c: Choice): boolean | null => (c === "inherit" ? null : c === "on");

/** Admin → Security scanning: the global switch, the mirror, and each environment. */
export function SecuritySettingsPage() {
  useSecurityLive();
  const q = useQuery(settingsQuery);
  return (
    <Page title="Security scanning" kicker="Enterprise">
      {q.data && <Settings s={q.data} />}
    </Page>
  );
}

function Settings({ s }: { s: SecuritySettings }) {
  const qc = useQueryClient();
  const set = (next: SecuritySettings) => qc.setQueryData(settingsQuery.queryKey, next);
  const mirror = useMirrorDraft(s.global.mirror);
  const [confirming, setConfirming] = useState(false);
  const image = s.global.mirror?.trivy_image ?? s.global.default_trivy_image;
  const save = useMutation({
    mutationFn: (enabled: boolean) => putSettings({ enabled, mirror: keepMirror(s.global.mirror) }),
    onSuccess: set,
    onError: toastError,
  });
  const saveMirror = useMutation({
    mutationFn: () => putSettings({ enabled: s.global.enabled, mirror: mirror.body() }),
    onSuccess: (next) => {
      set(next);
      void qc.invalidateQueries({ queryKey: securityKeys.all });
    },
    onError: toastError,
  });
  return (
    <div className="grid items-start gap-5 lg:grid-cols-2">
      <div className="grid gap-5">
        <Panel
          title="Scanning"
          icon={ShieldAlert}
          actions={
            s.global.enabled ? (
              <Button variant="outline" onClick={() => save.mutate(false)}>
                <PowerOff aria-hidden />
                Turn off
              </Button>
            ) : (
              <Button onClick={() => setConfirming(true)}>
                <Power aria-hidden />
                Turn on
              </Button>
            )
          }
        >
          <p className="text-sm text-ink-muted">
            {s.global.enabled
              ? "On for every environment that inherits this setting. Images are scanned when they arrive and once a day."
              : "Off. Nothing is pulled and nothing runs until you turn it on, here or for one environment."}
          </p>
        </Panel>
        <Panel title="Mirror" icon={HardDriveDownload}>
          <div className="grid gap-3">
            <p className="text-sm text-ink-muted">For air-gapped environments: where Trivy and its vulnerability databases come from. Leave empty for Trivy's public locations.</p>
            <MirrorFields draft={mirror} idPrefix="global-mirror" defaultImage={s.global.default_trivy_image} />
            <Button className="justify-self-start" disabled={saveMirror.isPending} onClick={() => saveMirror.mutate()}>
              <Save aria-hidden />
              Save mirror
            </Button>
          </div>
        </Panel>
      </div>
      <Panel title="Environments" icon={ServerCog} flush>
        <table aria-label="Scanning per environment" className="w-full text-sm">
          <thead className="text-left text-xs text-meta">
            <tr>
              <th className="px-4 py-2 font-normal">Environment</th>
              <th className="px-4 py-2 font-normal">Setting</th>
              <th className="px-4 py-2 font-normal">Scanner</th>
            </tr>
          </thead>
          <tbody>
            {s.envs.map((e) => (
              <EnvRow key={e.env} e={e} globalOn={s.global.enabled} onSaved={set} />
            ))}
          </tbody>
        </table>
      </Panel>
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title="Turn on security scanning?"
        description={
          <>
            FleetOps will pull the Trivy image (<code className="font-ident break-all">{image}</code>) and download its vulnerability database on every
            environment that inherits this setting.
          </>
        }
        confirmLabel="Turn on"
        onConfirm={() => save.mutateAsync(true)}
      />
    </div>
  );
}

function EnvRow({ e, globalOn, onSaved }: { e: EnvSettingsView; globalOn: boolean; onSaved: (s: SecuritySettings) => void }) {
  const change = useMutation({
    mutationFn: (c: Choice) =>
      putEnvSettings(e.env, {
        enabled: enabledOf(c),
        mirror: keepMirror(e.mirror),
        socket_path: e.socket_path ?? null,
      }),
    onSuccess: onSaved,
    onError: toastError,
  });
  return (
    <tr className="border-t border-border">
      <td className="px-4 py-2">
        <div className="font-medium">{e.env}</div>
        <div className="font-ident text-xs text-meta">{e.kind}</div>
      </td>
      <td className="px-4 py-2">
        <NativeSelect aria-label={`Scanning on ${e.env}`} value={choiceOf(e.enabled)} onChange={(ev) => change.mutate(ev.target.value as Choice)}>
          <option value="inherit">Inherit ({globalOn ? "on" : "off"})</option>
          <option value="on">On</option>
          <option value="off">Off</option>
        </NativeSelect>
      </td>
      <td className="px-4 py-2">
        <StateBadge state={scannerLabel(e.status)} tone={scannerTone(e.status)} />
      </td>
    </tr>
  );
}
