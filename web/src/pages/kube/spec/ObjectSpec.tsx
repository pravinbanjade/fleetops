import { Link } from "@tanstack/react-router";
import { ChevronsDown, Database, FileSearch, Pencil, Plus, Replace, RotateCcw, Tags, TextCursorInput, Undo2, Variable, X } from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";
import type { KubeKind, KubeRef } from "@/api/gen/types.gen";
import { ImageRef } from "@/components/ImageRef";
import { Masked } from "@/components/Masked";
import { Panel } from "@/components/Panel";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { formatAgo } from "@/lib/format";
import { isSecretKey } from "@/lib/secrets";
import { cn } from "@/lib/utils";
import { KIND_LABEL } from "../kinds";
import {
  activeTweaks,
  ENV_EDITABLE,
  envSource,
  readSpec,
  sameSpec,
  sourceText,
  validDataKey,
  validEnvName,
  type Container,
  type EnvSource,
  type EnvSpec,
  type ObjectSpec as Spec,
  type Op,
} from "./model";
import { TweakReview } from "./TweakReview";

type Target = { env: string; kind: KubeKind; ns: string; name: string };
type Review = { ops: Op[]; revert?: boolean; done?: () => void };

/**
 * What an object holds, read from its YAML: a ConfigMap's or Secret's data,
 * a workload's containers and their environment, its labels and
 * annotations. Data and env can be tweaked in place, reviewed as a dry run,
 * and reverted later from what the tweak recorded.
 */
export default function ObjectSpec({
  yaml,
  canWrite,
  owners,
  children,
  ...target
}: Target & { yaml: string; canWrite: boolean; owners: KubeRef[]; children?: ReactNode }) {
  const spec = useMemo(() => readSpec(target.kind, yaml), [target.kind, yaml]);
  const [review, setReview] = useState<Review | null>(null);
  const tweaks = useMemo(() => activeTweaks(spec), [spec]);
  return (
    <>
      {children}
      {spec.data && <DataPanel target={target} spec={spec} canWrite={canWrite} tweaks={tweaks} onReview={setReview} />}
      {spec.containers && spec.containers.length > 0 && (
        <EnvPanel target={target} spec={spec} canWrite={canWrite} owners={owners} tweaks={tweaks} onReview={setReview} />
      )}
      <MetaPanel spec={spec} />
      {review && (
        <TweakReview
          {...target}
          ops={review.ops}
          spec={spec}
          revert={review.revert}
          onClose={() => setReview(null)}
          onApplied={() => {
            review.done?.();
            setReview(null);
          }}
        />
      )}
    </>
  );
}

/* ── Shared pieces ─────────────────────────────────────────────────────── */

const envKey = (container: string, name: string) => `${container}\u0000${name}`;

type Added = { id: number; scope: string; name: string; value: string };

/** An edit in progress: changes to what exists, keyed by row, and new rows. */
function useDraft() {
  const [editing, setEditing] = useState(false);
  const [changes, setChanges] = useState<Map<string, Op>>(new Map());
  const [added, setAdded] = useState<Added[]>([]);
  const [nextId, setNextId] = useState(1);
  const reset = () => {
    setEditing(false);
    setChanges(new Map());
    setAdded([]);
  };
  return {
    editing,
    changes,
    added,
    start: () => setEditing(true),
    reset,
    set: (key: string, op: Op | null) =>
      setChanges((m) => {
        const next = new Map(m);
        if (op) next.set(key, op);
        else next.delete(key);
        return next;
      }),
    add: (scope: string) => {
      setAdded((a) => [...a, { id: nextId, scope, name: "", value: "" }]);
      setNextId((n) => n + 1);
    },
    patch: (id: number, p: Partial<Added>) => setAdded((a) => a.map((x) => (x.id === id ? { ...x, ...p } : x))),
    drop: (id: number) => setAdded((a) => a.filter((x) => x.id !== id)),
  };
}
type Draft = ReturnType<typeof useDraft>;

/** The sticky bar under a panel being edited: how much changed, Cancel, Review. */
function EditBar({ count, invalid, onCancel, onReview }: { count: number; invalid: boolean; onCancel: () => void; onReview: () => void }) {
  return (
    <div className="sticky bottom-0 z-10 flex flex-wrap items-center gap-x-3 gap-y-2 border-t bg-card/95 px-4 py-2.5 backdrop-blur-sm">
      <p className="text-sm text-muted-foreground" aria-live="polite">
        {invalid ? "Fix the highlighted names first." : count === 0 ? "No changes yet." : `${count} ${count === 1 ? "change" : "changes"}`}
      </p>
      <div className="ml-auto flex gap-2">
        <Button variant="outline" size="sm" onClick={onCancel}>
          <X aria-hidden />
          Cancel
        </Button>
        <Button size="sm" disabled={count === 0 || invalid} onClick={onReview}>
          <FileSearch aria-hidden />
          Review
        </Button>
      </div>
    </div>
  );
}

/** Says the object carries tweaks and offers to put them all back. */
function TweakBand({ spec, count, onRevertAll }: { spec: Spec; count: number; onRevertAll?: () => void }) {
  const t = spec.tweaks;
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b bg-muted/40 px-4 py-2">
      <StateBadge state="Tweaked" tone="idle" />
      <p className="min-w-0 text-sm">
        {t?.by ? `Changed by ${t.by}` : "Changed from FleetOps"}
        {t?.at ? `, ${formatAgo(t.at)}` : ""}. {count} {count === 1 ? "value differs" : "values differ"} from before.
      </p>
      {onRevertAll && (
        <Button variant="outline" size="xs" className="ml-auto" onClick={onRevertAll}>
          <RotateCcw aria-hidden />
          Revert all
        </Button>
      )}
    </div>
  );
}

/** A strip of the panel: a name cell boxed off by a rule, the value, then actions. */
function Strip({ name, children, actions, tone, field }: { name: ReactNode; children: ReactNode; actions?: ReactNode; tone?: "changed" | "removed"; field?: boolean }) {
  return (
    <li
      className={cn(
        "grid grid-cols-[minmax(0,1fr)_auto] border-b last:border-b-0 sm:grid-cols-[minmax(9rem,16rem)_minmax(0,1fr)_auto]",
        tone === "changed" && "bg-selected",
      )}
    >
      {/* Beside a 32px field the name drops 6px, so the two share a centre line. */}
      <div className={cn("min-w-0 px-4 pt-2.5 break-words sm:border-r sm:py-2.5", field && "sm:pt-4", tone === "removed" && "text-muted-foreground line-through")}>{name}</div>
      <div className="col-start-1 row-start-2 min-w-0 px-4 pt-1 pb-2.5 sm:col-start-2 sm:row-start-1 sm:py-2.5">{children}</div>
      <div className={cn("col-start-2 row-span-2 row-start-1 flex items-start gap-1 py-1.5 pr-2 sm:col-start-3 sm:row-span-1", field && "sm:pt-2.5")}>{actions}</div>
    </li>
  );
}

function Was({ children }: { children: ReactNode }) {
  return <p className="mt-1 text-xs break-words text-muted-foreground">Was {children}</p>;
}

/** What a value was: inline when one line, as a block when it is a file. */
function WasValue({ value }: { value: string }) {
  if (!value.includes("\n")) return <Was><Mono>{value || "(empty)"}</Mono></Was>;
  return (
    <div className="mt-1 text-xs text-muted-foreground">
      Was
      <pre className="mt-1 max-h-40 overflow-auto rounded-sm border px-3 py-2 font-ident leading-5 font-normal">{value.replace(/\n$/, "")}</pre>
    </div>
  );
}

function IconAction({ label, icon: Icon, onClick }: { label: string; icon: typeof X; onClick: () => void }) {
  return (
    <Button variant="ghost" size="icon-sm" aria-label={label} title={label} onClick={onClick}>
      <Icon aria-hidden />
    </Button>
  );
}

const Mono = ({ children, className }: { children: ReactNode; className?: string }) => (
  <span className={cn("font-ident font-normal whitespace-pre-wrap [overflow-wrap:anywhere]", className)}>{children}</span>
);

/* ── Environment ───────────────────────────────────────────────────────── */

function EnvPanel({
  target,
  spec,
  canWrite,
  owners,
  tweaks,
  onReview,
}: {
  target: Target;
  spec: Spec;
  canWrite: boolean;
  owners: KubeRef[];
  tweaks: Op[];
  onReview: (r: Review) => void;
}) {
  const draft = useDraft();
  const editable = canWrite && ENV_EDITABLE.includes(target.kind);
  const containers = spec.containers!;
  const envTweaks = tweaks.filter((t) => t.op === "env");

  const named = (a: Added) => a.name.trim();
  const problems = new Map<number, string>();
  for (const a of draft.added) {
    const c = containers.find((x) => x.name === a.scope);
    const n = named(a);
    const taken =
      c?.env.some((e) => e.name === n && draft.changes.get(envKey(c.name, n))?.to !== null) ||
      draft.added.some((o) => o.id !== a.id && o.scope === a.scope && named(o) === n);
    if (!n) problems.set(a.id, "Name it");
    else if (!validEnvName(n)) problems.set(a.id, "Letters, digits, _ . and - only, not starting with a digit");
    else if (taken) problems.set(a.id, `${n} is already set here`);
  }
  const ops: Op[] = [
    ...[...draft.changes.values()].filter((op) => {
      if (op.op !== "env") return false;
      const now = containers.find((c) => c.name === op.container)?.env.findLast((e) => e.name === op.name)?.spec ?? null;
      return !sameSpec(now, op.to);
    }),
    ...draft.added.filter((a) => !problems.has(a.id)).map((a): Op => ({ op: "env", container: a.scope, name: named(a), to: { value: a.value } })),
  ];

  const aside = !editable && canWrite ? <ReadOnlyWhy env={target.env} kind={target.kind} ns={target.ns} owners={owners} /> : undefined;
  return (
    <Panel
      title="Environment"
      icon={Variable}
      aside={aside}
      flush
      actions={
        editable &&
        !draft.editing && (
          <Button variant="outline" size="sm" onClick={draft.start}>
            <Pencil aria-hidden />
            Edit env
          </Button>
        )
      }
    >
      {envTweaks.length > 0 && !draft.editing && (
        <TweakBand spec={spec} count={envTweaks.length} onRevertAll={editable ? () => onReview({ ops: envTweaks, revert: true }) : undefined} />
      )}
      <div className="[&>*:last-child]:border-b-0">
      {containers.map((c) => (
        <ContainerEnv
          key={c.name}
          target={target}
          c={c}
          many={containers.length > 1}
          draft={draft}
          problems={problems}
          tweaks={envTweaks}
          onRevert={editable ? (op) => onReview({ ops: [op], revert: true }) : undefined}
        />
      ))}
      </div>
      {draft.editing && (
        <EditBar
          count={ops.length}
          invalid={problems.size > 0}
          onCancel={draft.reset}
          onReview={() => onReview({ ops, done: draft.reset })}
        />
      )}
    </Panel>
  );
}

/** Why a pod's environment is not edited here, and where it is. */
function ReadOnlyWhy({ env, kind, ns, owners }: { env: string; kind: KubeKind; ns: string; owners: KubeRef[] }) {
  const owner = [...owners].reverse().find((o) => o.link && ENV_EDITABLE.includes(o.link));
  if (owner?.link)
    return (
      <>
        Set by{" "}
        <Link to="/envs/$env/k8s/$kind/$ns/$name" params={{ env, kind: owner.link, ns, name: owner.name }} className="text-foreground underline decoration-border underline-offset-4 hover:decoration-primary">
          {owner.kind} <span className="font-ident">{owner.name}</span>
        </Link>
        ; change it there.
      </>
    );
  if (kind === "jobs") return <>A Job’s pod template cannot change once it is created.</>;
  if (kind === "pods") return <>A pod’s environment cannot change while it exists.</>;
  return undefined;
}

function ContainerEnv({
  target,
  c,
  many,
  draft,
  problems,
  tweaks,
  onRevert,
}: {
  target: Target;
  c: Container;
  many: boolean;
  draft: Draft;
  problems: Map<number, string>;
  tweaks: Op[];
  onRevert?: (op: Op) => void;
}) {
  const added = draft.added.filter((a) => a.scope === c.name);
  const empty = c.env.length === 0 && c.envFrom.length === 0 && added.length === 0;
  return (
    <div className="border-b" role="group" aria-label={`Container ${c.name}`}>
      {(many || c.image) && (
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 border-b bg-muted/40 px-4 py-2 text-sm">
          <span className="font-ident font-semibold">{c.name}</span>
          {c.init && <StateBadge state="init" tone="idle" />}
          {c.image && (
            <span className="ml-auto min-w-0 truncate text-xs text-muted-foreground">
              <ImageRef value={c.image} />
            </span>
          )}
        </div>
      )}
      <ul aria-label={`Environment of ${c.name}`} className="font-strip text-sm">
        {c.env.map((e) => {
          const key = envKey(c.name, e.name);
          const change = draft.changes.get(key);
          const tweak = tweaks.find((t) => t.op === "env" && t.container === c.name && t.name === e.name);
          return (
            <EnvRow
              key={key}
              target={target}
              name={e.name}
              spec={e.spec}
              source={e.source}
              editing={draft.editing}
              change={change?.op === "env" ? change.to : undefined}
              changed={!!change}
              onChange={(to) => draft.set(key, to === undefined ? null : { op: "env", container: c.name, name: e.name, to })}
              tweak={tweak?.op === "env" && !draft.editing ? tweak : undefined}
              onRevert={onRevert}
            />
          );
        })}
        {!draft.editing &&
          tweaks.flatMap((t) =>
            t.op === "env" && t.container === c.name && t.to !== null && !c.env.some((e) => e.name === t.name)
              ? [
                  <Strip
                    key={`gone/${t.name}`}
                    name={<Mono className="font-medium">{t.name}</Mono>}
                    tone="removed"
                    actions={onRevert && <IconAction label={`Revert ${t.name}`} icon={RotateCcw} onClick={() => onRevert(t)} />}
                  >
                    <span className="text-muted-foreground">Removed</span>
                    <Was>{t.to.valueFrom ? sourceText(envSource(t.to)) : <Mono>{t.to.value || "(empty)"}</Mono>}</Was>
                  </Strip>,
                ]
              : [],
          )}
        {c.envFrom.map((f) => (
          <Strip key={`${f.kind}/${f.name}/${f.prefix ?? ""}`} name={<span className="text-muted-foreground">All keys{f.prefix && <>, prefixed <Mono className="text-foreground">{f.prefix}</Mono></>}</span>}>
            <span className="text-muted-foreground">From </span>
            <ObjectLink target={target} kind={f.kind} name={f.name} />
          </Strip>
        ))}
        {added.map((a) => {
          const problem = problems.get(a.id);
          const id = `env-new-${a.id}`;
          return (
            <Strip
              key={a.id}
              tone="changed"
              name={
                <>
                  <Input
                    aria-label="New variable name"
                    aria-invalid={!!problem && a.name !== "" ? true : undefined}
                    aria-describedby={problem ? `${id}-err` : undefined}
                    placeholder="NAME"
                    autoFocus
                    spellCheck={false}
                    autoComplete="off"
                    className="h-8 font-ident"
                    value={a.name}
                    onChange={(ev) => draft.patch(a.id, { name: ev.target.value })}
                  />
                  {problem && a.name !== "" && (
                    <p id={`${id}-err`} className="mt-1 text-xs text-destructive">
                      {problem}
                    </p>
                  )}
                </>
              }
              actions={<IconAction label="Discard new variable" icon={X} onClick={() => draft.drop(a.id)} />}
            >
              <Input
                aria-label={`Value of ${a.name || "new variable"}`}
                placeholder="value"
                spellCheck={false}
                autoComplete="off"
                className="h-8 font-ident font-normal"
                value={a.value}
                onChange={(ev) => draft.patch(a.id, { value: ev.target.value })}
              />
            </Strip>
          );
        })}
        {empty && <li className="px-4 py-2.5 text-muted-foreground">No environment variables.</li>}
      </ul>
      {draft.editing && (
        <div className="border-t px-2 py-1.5">
          <Button variant="ghost" size="sm" onClick={() => draft.add(c.name)}>
            <Plus aria-hidden />
            Add variable{many && <span className="sr-only"> to {c.name}</span>}
          </Button>
        </div>
      )}
    </div>
  );
}

/** One env var: its value or where the value comes from, editable while editing. */
function EnvRow({
  target,
  name,
  spec,
  source,
  editing,
  change,
  changed,
  onChange,
  tweak,
  onRevert,
}: {
  target: Target;
  name: string;
  spec: EnvSpec;
  source: EnvSource;
  editing: boolean;
  /** The drafted spec; null when drafted for removal. */
  change: EnvSpec | null | undefined;
  changed: boolean;
  /** Undefined puts the row back as it is. */
  onChange: (to: EnvSpec | null | undefined) => void;
  tweak?: Extract<Op, { op: "env" }>;
  onRevert?: (op: Op) => void;
}) {
  const label = <Mono className="font-medium">{name}</Mono>;
  const current = <SourceValue target={target} name={name} source={source} />;
  if (editing && changed && change === null)
    return (
      <Strip name={label} tone="removed" actions={<IconAction label={`Keep ${name}`} icon={Undo2} onClick={() => onChange(undefined)} />}>
        <span className="text-muted-foreground">Will be removed</span>
      </Strip>
    );
  if (editing) {
    const literal = changed ? change?.valueFrom === undefined : source.kind === "value";
    const differs = changed && !sameSpec(change, spec);
    return (
      <Strip
        name={label}
        field={literal}
        tone={differs ? "changed" : undefined}
        actions={
          <>
            {differs && <IconAction label={`Undo change to ${name}`} icon={Undo2} onClick={() => onChange(undefined)} />}
            <IconAction label={`Remove ${name}`} icon={X} onClick={() => onChange(null)} />
          </>
        }
      >
        {literal ? (
          <>
            <Input
              aria-label={`Value of ${name}`}
              spellCheck={false}
              autoComplete="off"
              className="h-8 font-ident font-normal"
              value={changed ? (change?.value ?? "") : (spec.value ?? "")}
              onChange={(ev) => onChange({ value: ev.target.value })}
            />
            {differs && <Was>{source.kind === "value" ? <Mono>{source.value || "(empty)"}</Mono> : sourceText(source)}</Was>}
          </>
        ) : (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <span className="min-w-0">{current}</span>
            <Button variant="link" size="xs" className="text-xs" onClick={() => onChange({ value: "" })}>
              <TextCursorInput aria-hidden />
              Set a value instead<span className="sr-only"> for {name}</span>
            </Button>
          </div>
        )}
      </Strip>
    );
  }
  const was = tweak?.to;
  return (
    <Strip
      name={label}
      actions={tweak && onRevert && <IconAction label={`Revert ${name}`} icon={RotateCcw} onClick={() => onRevert(tweak)} />}
    >
      {current}
      {tweak && <Was>{was === null ? "not set" : was?.valueFrom ? sourceText(envSource(was)) : <Mono>{was?.value || "(empty)"}</Mono>}</Was>}
    </Strip>
  );
}

function SourceValue({ target, name, source }: { target: Target; name: string; source: EnvSource }) {
  switch (source.kind) {
    case "value":
      return source.value === "" ? (
        <span className="text-muted-foreground">(empty)</span>
      ) : (
        <Mono>{isSecretKey(name) ? <Masked name={name} value={source.value} /> : source.value}</Mono>
      );
    case "configmaps":
    case "secrets":
      return (
        <span>
          <span className="text-muted-foreground">From </span>
          <ObjectLink target={target} kind={source.kind} name={source.name} />
          <span className="text-muted-foreground">, key </span>
          <Mono>{source.key}</Mono>
        </span>
      );
    case "field":
      return (
        <span>
          <span className="text-muted-foreground">Field </span>
          <Mono>{source.path}</Mono>
        </span>
      );
    case "resource":
      return (
        <span>
          <span className="text-muted-foreground">Resource </span>
          <Mono>{source.resource}</Mono>
        </span>
      );
    default:
      return <span className="text-muted-foreground">Set from another source; see the YAML.</span>;
  }
}

function ObjectLink({ target, kind, name }: { target: Target; kind: "configmaps" | "secrets"; name: string }) {
  return (
    <Link
      to="/envs/$env/k8s/$kind/$ns/$name"
      params={{ env: target.env, kind, ns: target.ns, name }}
      className="underline decoration-border underline-offset-4 hover:decoration-primary"
    >
      {KIND_LABEL[kind].replace(/s$/, "")} <Mono>{name}</Mono>
    </Link>
  );
}

/* ── Data ──────────────────────────────────────────────────────────────── */

function DataPanel({
  target,
  spec,
  canWrite,
  tweaks,
  onReview,
}: {
  target: Target;
  spec: Spec;
  canWrite: boolean;
  tweaks: Op[];
  onReview: (r: Review) => void;
}) {
  const draft = useDraft();
  const secret = target.kind === "secrets";
  const editable = canWrite && !spec.immutable;
  const data = spec.data!;
  const dataTweaks = tweaks.filter((t) => t.op === "data");

  const problems = new Map<number, string>();
  for (const a of draft.added) {
    const k = a.name.trim();
    const taken = data.some((d) => d.key === k && draft.changes.get(k)?.to !== null) || draft.added.some((o) => o.id !== a.id && o.name.trim() === k);
    if (!k) problems.set(a.id, "Name the key");
    else if (!validDataKey(k)) problems.set(a.id, "Letters, digits, _ . and - only");
    else if (taken) problems.set(a.id, `${k} already exists`);
  }
  const ops: Op[] = [
    ...[...draft.changes.values()].filter((op) => op.op === "data" && (op.to === null || op.to !== data.find((d) => d.key === op.key)?.value)),
    ...draft.added.filter((a) => !problems.has(a.id)).map((a): Op => ({ op: "data", key: a.name.trim(), to: a.value })),
  ];

  return (
    <Panel
      title="Data"
      icon={Database}
      aside={spec.immutable ? "Immutable: it cannot change" : undefined}
      flush
      actions={
        editable &&
        !draft.editing && (
          <Button variant="outline" size="sm" onClick={draft.start}>
            <Pencil aria-hidden />
            Edit data
          </Button>
        )
      }
    >
      {dataTweaks.length > 0 && !draft.editing && (
        <TweakBand spec={spec} count={dataTweaks.length} onRevertAll={editable ? () => onReview({ ops: dataTweaks, revert: true }) : undefined} />
      )}
      <ul aria-label="Data" className="font-strip text-sm">
        {data.map((d) => {
          const change = draft.changes.get(d.key);
          const to = change?.op === "data" ? change.to : undefined;
          const tweak = !draft.editing ? dataTweaks.find((t) => t.op === "data" && t.key === d.key) : undefined;
          const label = <Mono className="font-medium">{d.key}</Mono>;
          const set = (v: string | null | undefined) => draft.set(d.key, v === undefined ? null : { op: "data", key: d.key, to: v });
          if (d.bytes !== undefined)
            return (
              <Strip key={d.key} name={label}>
                <span className="text-muted-foreground">Binary, {d.bytes} bytes</span>
              </Strip>
            );
          if (draft.editing && change && to === null)
            return (
              <Strip key={d.key} name={label} tone="removed" actions={<IconAction label={`Keep ${d.key}`} icon={Undo2} onClick={() => set(undefined)} />}>
                <span className="text-muted-foreground">Will be removed</span>
              </Strip>
            );
          if (draft.editing) {
            const differs = change !== undefined && to !== d.value;
            return (
              <Strip
                key={d.key}
                name={label}
                field={!secret || change !== undefined}
                tone={differs ? "changed" : undefined}
                actions={
                  <>
                    {differs && <IconAction label={`Undo change to ${d.key}`} icon={Undo2} onClick={() => set(undefined)} />}
                    <IconAction label={`Remove ${d.key}`} icon={X} onClick={() => set(null)} />
                  </>
                }
              >
                {secret && change === undefined ? (
                  <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                    <span className="text-muted-foreground">Hidden</span>
                    <Button variant="link" size="xs" className="text-xs" onClick={() => set("")}>
                      <Replace aria-hidden />
                      Replace value<span className="sr-only"> of {d.key}</span>
                    </Button>
                  </div>
                ) : (
                  <>
                    <ValueField label={`Value of ${d.key}`} value={to ?? d.value ?? ""} placeholder={secret ? "New value" : undefined} onChange={(v) => set(v)} />
                    {differs && !secret && <WasValue value={d.value ?? ""} />}
                    {secret && <p className="mt-1 text-xs text-muted-foreground">Replaces the current value.</p>}
                  </>
                )}
              </Strip>
            );
          }
          return (
            <Strip key={d.key} name={label} actions={tweak && editable && <IconAction label={`Revert ${d.key}`} icon={RotateCcw} onClick={() => onReview({ ops: [tweak], revert: true })} />}>
              {d.value === null ? <span className="text-muted-foreground">Hidden</span> : <DataValue value={d.value} />}
              {tweak?.op === "data" && (tweak.to === null ? <Was>not set</Was> : <WasValue value={tweak.to} />)}
            </Strip>
          );
        })}
        {!draft.editing &&
          dataTweaks.flatMap((t) =>
            t.op === "data" && t.to !== null && !data.some((d) => d.key === t.key)
              ? [
                  <Strip
                    key={`gone/${t.key}`}
                    name={<Mono className="font-medium">{t.key}</Mono>}
                    tone="removed"
                    actions={editable && <IconAction label={`Revert ${t.key}`} icon={RotateCcw} onClick={() => onReview({ ops: [t], revert: true })} />}
                  >
                    <span className="text-muted-foreground">Removed</span>
                    <WasValue value={t.to} />
                  </Strip>,
                ]
              : [],
          )}
        {draft.added.map((a) => {
          const problem = problems.get(a.id);
          return (
            <Strip
              key={a.id}
              tone="changed"
              name={
                <>
                  <Input
                    aria-label="New key"
                    aria-invalid={!!problem && a.name !== "" ? true : undefined}
                    placeholder="key"
                    autoFocus
                    spellCheck={false}
                    autoComplete="off"
                    className="h-8 font-ident"
                    value={a.name}
                    onChange={(ev) => draft.patch(a.id, { name: ev.target.value })}
                  />
                  {problem && a.name !== "" && <p className="mt-1 text-xs text-destructive">{problem}</p>}
                </>
              }
              actions={<IconAction label="Discard new key" icon={X} onClick={() => draft.drop(a.id)} />}
            >
              <ValueField label={`Value of ${a.name || "new key"}`} value={a.value} placeholder="value" onChange={(v) => draft.patch(a.id, { value: v })} />
            </Strip>
          );
        })}
        {data.length === 0 && draft.added.length === 0 && <li className="px-4 py-2.5 text-muted-foreground">No keys.</li>}
      </ul>
      {draft.editing && (
        <>
          <div className="border-t px-2 py-1.5">
            <Button variant="ghost" size="sm" onClick={() => draft.add("data")}>
              <Plus aria-hidden />
              Add key
            </Button>
          </div>
          <EditBar count={ops.length} invalid={problems.size > 0} onCancel={draft.reset} onReview={() => onReview({ ops, done: draft.reset })} />
        </>
      )}
    </Panel>
  );
}

/** A value as the file it often is: whole lines, scrolling past 16rem. */
function DataValue({ value }: { value: string }) {
  if (value === "") return <span className="text-muted-foreground">(empty)</span>;
  if (!value.includes("\n")) return <Mono>{value}</Mono>;
  return (
    <pre className="max-h-64 overflow-auto rounded-sm border bg-background/60 px-3 py-2 font-ident text-xs leading-5 font-normal">{value.replace(/\n$/, "")}</pre>
  );
}

/** A value field that grows with its lines, so a properties file edits as one. */
function ValueField({ label, value, placeholder, onChange }: { label: string; value: string; placeholder?: string; onChange: (v: string) => void }) {
  return (
    <Textarea
      aria-label={label}
      placeholder={placeholder}
      spellCheck={false}
      autoComplete="off"
      rows={1}
      className="max-h-80 min-h-8 resize-none py-1.5 font-ident text-[0.8125rem] leading-5 font-normal md:text-[0.8125rem]"
      value={value}
      onChange={(ev) => onChange(ev.target.value)}
    />
  );
}

/* ── Labels and annotations ────────────────────────────────────────────── */

function MetaPanel({ spec }: { spec: Spec }) {
  const [all, setAll] = useState(false);
  if (spec.labels.length === 0 && spec.annotations.length === 0) return null;
  const shown = all ? spec.annotations : spec.annotations.slice(0, 4);
  return (
    <Panel title="Labels and annotations" icon={Tags} flush>
      <ul className="font-strip text-sm">
        {spec.labels.length > 0 && (
          <Strip name={<span className="text-muted-foreground">Labels</span>}>
            <span className="flex flex-wrap gap-1.5">
              {spec.labels.map(([k, v]) => (
                <span key={k} className="inline-flex max-w-full rounded-xs border bg-background/60 px-1.5 font-ident text-xs leading-5 font-normal">
                  <span className="truncate text-muted-foreground">{k}=</span>
                  <span className="truncate">{v}</span>
                </span>
              ))}
            </span>
          </Strip>
        )}
        {spec.annotations.length > 0 && (
          <Strip name={<span className="text-muted-foreground">Annotations</span>}>
            <dl className="grid gap-2">
              {shown.map(([k, v]) => (
                <div key={k} className="min-w-0 font-ident text-xs leading-5 font-normal">
                  <dt className="break-all text-muted-foreground">{k}</dt>
                  <dd className="line-clamp-3 break-all whitespace-pre-wrap">{v}</dd>
                </div>
              ))}
            </dl>
            {spec.annotations.length > 4 && (
              <Button variant="link" size="xs" className="mt-2 text-xs" aria-expanded={all} onClick={() => setAll(!all)}>
                <ChevronsDown aria-hidden />
                {all ? "Show fewer" : `Show all ${spec.annotations.length}`}
              </Button>
            )}
          </Strip>
        )}
      </ul>
    </Panel>
  );
}
