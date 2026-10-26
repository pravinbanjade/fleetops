import { ChevronDown, ChevronUp, Download, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { toast } from "sonner";
import type { LogLine } from "@/api/gen/types.gen";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { saveText } from "@/lib/download";
import { compileQuery, LogBuffer, LogMatches, logText } from "@/lib/logs";
import { useLive } from "@/live/LiveProvider";
import { LogView } from "./LogView";
import { DEFAULT_QUERY, useLogStream, type LogQuery } from "./useLogStream";

const TAILS = [
  ["100", "Last 100 lines"],
  ["1000", "Last 1,000 lines"],
  ["10000", "Last 10,000 lines"],
  ["all", "All lines"],
] as const;

/** Seconds back from now, as select values. */
const SINCE = [
  ["", "Any time"],
  ["300", "Last 5 minutes"],
  ["3600", "Last hour"],
  ["86400", "Last 24 hours"],
] as const;

/**
 * The container's log, live: tail and time range, streams, timestamps,
 * wrapping and follow, search, and download.
 */
export function LogsTab({
  env,
  id,
  name,
  running,
  startedAt,
  container,
  podPrefix,
  canPrevious,
}: {
  env: string;
  id: string;
  name: string;
  running: boolean;
  startedAt?: string;
  /** Kubernetes only: the pod's container. */
  container?: string;
  /** Merged Kubernetes logs: the workload's name and a dash, dropped from pod suffixes. */
  podPrefix?: string;
  /** Kubernetes only: whether a previous run can be read; absent hides the toggle. */
  canPrevious?: boolean;
}) {
  const [query, setQuery] = useState<LogQuery>(() => {
    // A workload reads every pod: fewer lines each (spec 3i §2).
    const base = podPrefix !== undefined ? { ...DEFAULT_QUERY, tail: 100 } : DEFAULT_QUERY;
    return container ? { ...base, container } : base;
  });
  const [since, setSince] = useState("");
  const [timestamps, setTimestamps] = useState(false);
  const [wrap, setWrap] = useState(false);
  const [follow, setFollow] = useState(true);
  const { buffer, version, state, clear } = useLogStream(env, id, query, startedAt);
  const change = (q: Partial<LogQuery>) => setQuery({ ...query, ...q });
  const search = useSearch(buffer, version);
  const jump = (dir: 1 | -1) => {
    if (search.jump(dir) !== null) setFollow(false);
  };

  return (
    <div className="grid gap-3">
      <div className="flex flex-wrap items-center gap-x-5 gap-y-2">
        <NativeSelect
          aria-label="Lines"
          className="w-44"
          value={query.tail === null ? "all" : String(query.tail)}
          onChange={(e) => change({ tail: e.target.value === "all" ? null : Number(e.target.value) })}
        >
          {TAILS.map(([v, label]) => (
            <option key={v} value={v}>
              {label}
            </option>
          ))}
        </NativeSelect>
        <NativeSelect
          aria-label="Since"
          className="w-40"
          value={since}
          onChange={(e) => {
            setSince(e.target.value);
            change({ since: e.target.value ? Math.floor(Date.now() / 1000) - Number(e.target.value) : null });
          }}
        >
          {SINCE.map(([v, label]) => (
            <option key={v} value={v}>
              {label}
            </option>
          ))}
        </NativeSelect>
        <Toggle id="logs-stdout" checked={query.stdout} onChange={(stdout) => change({ stdout })}>
          stdout
        </Toggle>
        <Toggle id="logs-stderr" checked={query.stderr} onChange={(stderr) => change({ stderr })}>
          stderr
        </Toggle>
        <Toggle id="logs-timestamps" checked={timestamps} onChange={setTimestamps}>
          Timestamps
        </Toggle>
        <Toggle id="logs-wrap" checked={wrap} onChange={setWrap}>
          Wrap lines
        </Toggle>
        <Toggle id="logs-follow" checked={follow} onChange={setFollow}>
          Follow
        </Toggle>
        {canPrevious !== undefined && (
          <Toggle
            id="logs-previous"
            checked={!!query.previous}
            disabled={!canPrevious}
            onChange={(previous) => {
              change({ previous });
              if (previous) setFollow(false);
            }}
          >
            Previous run
          </Toggle>
        )}
        <Button variant="outline" size="sm" onClick={clear}>
          <X aria-hidden />
          Clear
        </Button>
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <Input
          aria-label="Search logs"
          placeholder="Search"
          className="w-64"
          value={search.text}
          onChange={(e) => search.setText(e.target.value)}
          onKeyDown={(e: KeyboardEvent) => {
            if (e.key !== "Enter") return;
            e.preventDefault();
            jump(e.shiftKey ? -1 : 1);
          }}
        />
        <Toggle id="logs-regex" checked={search.regex} onChange={search.setRegex}>
          Regex
        </Toggle>
        <Button variant="outline" size="icon" aria-label="Previous match" disabled={!search.seqs.length} onClick={() => jump(-1)}>
          <ChevronUp />
        </Button>
        <Button variant="outline" size="icon" aria-label="Next match" disabled={!search.seqs.length} onClick={() => jump(1)}>
          <ChevronDown />
        </Button>
        <span aria-live="polite" className={search.error ? "text-sm text-destructive" : "text-sm text-muted-foreground"}>
          {search.summary}
        </span>
        <div className="ml-auto flex gap-2">
          <Button variant="outline" size="sm" onClick={() => saveText(`${name}.log`, logText(buffer.entries(), timestamps))}>
            <Download /> Download shown
          </Button>
          <FullDownload env={env} id={id} name={name} query={query} timestamps={timestamps} />
        </div>
      </div>
      <LogStatus state={state} running={running} empty={buffer.length === 0} />
      <LogView
        buffer={buffer}
        version={version}
        timestamps={timestamps}
        wrap={wrap}
        follow={follow}
        onFollowChange={setFollow}
        highlight={search.re}
        current={search.current}
        podPrefix={podPrefix}
      />
    </div>
  );
}

function Toggle({
  id,
  checked,
  disabled,
  onChange,
  children,
}: {
  id: string;
  checked: boolean;
  disabled?: boolean;
  onChange(v: boolean): void;
  children: ReactNode;
}) {
  return (
    <div className="flex items-center gap-2">
      <Checkbox id={id} checked={checked} disabled={disabled} onCheckedChange={(v) => onChange(v === true)} />
      <Label htmlFor={id} className="font-normal">
        {children}
      </Label>
    </div>
  );
}

function LogStatus({ state, running, empty }: { state: ReturnType<typeof useLogStream>["state"]; running: boolean; empty: boolean }) {
  if (state.state === "error") {
    return (
      <p role="alert" className="text-sm text-destructive">
        {state.message}
      </p>
    );
  }
  if (!running) {
    return (
      <p className="text-sm text-muted-foreground">
        The container is not running. New lines appear here when it starts again.
      </p>
    );
  }
  if (state.state === "ended") return <p className="text-sm text-muted-foreground">The log stream ended.</p>;
  if (empty) return <p className="text-sm text-muted-foreground">No log lines yet.</p>;
  return null;
}

/**
 * The search box's state: the matching lines' seqs, kept current as lines
 * arrive, and the match the user has moved to.
 */
function useSearch(buffer: LogBuffer, version: number) {
  const [text, setText] = useState("");
  const [regex, setRegex] = useState(false);
  const [current, setCurrent] = useState<{ text: string; regex: boolean; seq: number } | null>(null);
  const compiled = useMemo(() => compileQuery(text, regex), [text, regex]);
  const re = compiled instanceof RegExp ? compiled : null;
  const matches = useMemo(() => new LogMatches(buffer), [buffer]);
  // eslint-disable-next-line react-hooks/exhaustive-deps -- `version` marks new lines in the buffer
  const seqs = useMemo(() => [...matches.sync(re)], [matches, re, version]);
  const seq = current && current.text === text && current.regex === regex ? current.seq : null;
  const index = seq === null ? -1 : seqs.indexOf(seq);

  /** Moves to the next (1) or previous (-1) match, wrapping around; returns its seq. */
  const jump = (dir: 1 | -1) => {
    if (!seqs.length) return null;
    let next: number;
    if (seq === null) next = dir === 1 ? seqs[0] : seqs[seqs.length - 1];
    else if (dir === 1) next = seqs.find((s) => s > seq) ?? seqs[0];
    else next = seqs.findLast((s) => s < seq) ?? seqs[seqs.length - 1];
    setCurrent({ text, regex, seq: next });
    return next;
  };

  const error = compiled && !(compiled instanceof RegExp) ? compiled.error : null;
  const summary = error
    ? "Invalid regular expression"
    : !re
      ? ""
      : !seqs.length
        ? "No matches"
        : index >= 0
          ? `${index + 1} of ${seqs.length}`
          : `${seqs.length} ${seqs.length === 1 ? "match" : "matches"}`;
  return { text, setText, regex, setRegex, re, seqs, current: seq, jump, error, summary };
}

/** Fetches the whole log (not only the lines held) and saves it. */
function FullDownload({
  env,
  id,
  name,
  query,
  timestamps,
}: {
  env: string;
  id: string;
  name: string;
  query: LogQuery;
  timestamps: boolean;
}) {
  const { ws } = useLive();
  const [busy, setBusy] = useState(false);
  // Leaving the page abandons a download still in progress.
  const pending = useRef<(() => void) | null>(null);
  useEffect(() => () => pending.current?.(), []);
  const start = () => {
    if (!ws) return;
    setBusy(true);
    const all = new LogBuffer(Infinity);
    let skipped = 0;
    const finish = () => {
      pending.current = null;
      setBusy(false);
    };
    pending.current = ws.subscribe(
      {
        topic: "logs",
        env,
        id,
        opts: { follow: false, tail: null, since: null, timestamps: true, stdout: query.stdout, stderr: query.stderr },
      },
      {
        onData: (d) => all.append(d as LogLine[]),
        onGap: (n) => {
          skipped += n;
          all.gap(n);
        },
        onEnd: () => {
          finish();
          saveText(`${name}.log`, logText(all.entries(), timestamps));
          if (skipped) toast.warning(`Saved the log, but ${skipped} lines were skipped because the browser fell behind.`);
        },
        onError: (m) => {
          finish();
          toast.error(`Could not download the log: ${m.message}`);
        },
        onLost: () => {
          finish();
          toast.error("The connection dropped, so the download was cancelled.");
        },
      },
      // Sent again after a reconnect, it would append the whole log a second time.
      { resubscribe: false },
    ).unsubscribe;
  };
  return (
    <Button variant="outline" size="sm" disabled={busy} onClick={start}>
      <Download /> {busy ? "Preparing…" : "Download full log"}
    </Button>
  );
}
