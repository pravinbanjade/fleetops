import { useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
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

/** The container's log, live: tail and time range, streams, timestamps, wrapping and follow. */
export function LogsTab({ env, id, running }: { env: string; id: string; running: boolean }) {
  const [query, setQuery] = useState<LogQuery>(DEFAULT_QUERY);
  const [since, setSince] = useState("");
  const [timestamps, setTimestamps] = useState(false);
  const [wrap, setWrap] = useState(false);
  const [follow, setFollow] = useState(true);
  const { buffer, version, state, clear } = useLogStream(env, id, query, running);
  const change = (q: Partial<LogQuery>) => setQuery({ ...query, ...q });

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
        <Button variant="outline" size="sm" onClick={clear}>
          Clear
        </Button>
      </div>
      <LogStatus state={state} running={running} empty={buffer.length === 0} />
      <LogView
        buffer={buffer}
        version={version}
        timestamps={timestamps}
        wrap={wrap}
        follow={follow}
        onFollowChange={setFollow}
      />
    </div>
  );
}

function Toggle({
  id,
  checked,
  onChange,
  children,
}: {
  id: string;
  checked: boolean;
  onChange(v: boolean): void;
  children: ReactNode;
}) {
  return (
    <div className="flex items-center gap-2">
      <Checkbox id={id} checked={checked} onCheckedChange={(v) => onChange(v === true)} />
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
