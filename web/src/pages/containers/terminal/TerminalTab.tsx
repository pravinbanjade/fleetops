import { Plug, Unplug } from "lucide-react";
import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { splitArgs } from "@/lib/args";
import { TerminalView, type SessionEnd } from "./TerminalView";

type Shell = "auto" | "bash" | "sh" | "custom";

interface Session {
  /** A new number mounts a new terminal. */
  n: number;
  cmd: string[] | null;
  user: string | null;
}

/**
 * A shell in the container. Nothing runs until the user connects: every
 * exec is audited, and switching tabs should not start one.
 */
export function TerminalTab({
  env,
  id,
  running,
  container,
}: {
  env: string;
  id: string;
  running: boolean;
  /** Kubernetes only: the pod's container. */
  container?: string;
}) {
  const [shell, setShell] = useState<Shell>("auto");
  const [command, setCommand] = useState("");
  const [user, setUser] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [session, setSession] = useState<Session | null>(null);
  const [started, setStarted] = useState(false);
  const [ended, setEnded] = useState<SessionEnd | null>(null);

  const connect = (e?: FormEvent) => {
    e?.preventDefault();
    let cmd: string[] | null = null;
    if (shell === "bash" || shell === "sh") cmd = [shell];
    if (shell === "custom") {
      try {
        cmd = splitArgs(command);
      } catch (err) {
        setFormError(`Command: ${(err as Error).message}`);
        return;
      }
      if (!cmd.length) {
        setFormError("Command: enter the command to run");
        return;
      }
    }
    setFormError(null);
    setStarted(false);
    setEnded(null);
    setSession({ n: (session?.n ?? 0) + 1, cmd, user: user.trim() || null });
  };

  const live = session && !ended;
  return (
    <div className="grid gap-3">
      <form onSubmit={connect} className="flex flex-wrap items-end gap-3">
        <div className="grid gap-1.5">
          <Label htmlFor="term-shell">Shell</Label>
          <NativeSelect
            id="term-shell"
            className="w-52"
            value={shell}
            disabled={!!live}
            onChange={(e) => setShell(e.target.value as Shell)}
          >
            <option value="auto">Auto (bash, then sh)</option>
            <option value="bash">bash</option>
            <option value="sh">sh</option>
            <option value="custom">Custom command</option>
          </NativeSelect>
        </div>
        {shell === "custom" && (
          <div className="grid gap-1.5">
            <Label htmlFor="term-command">Command</Label>
            <Input
              id="term-command"
              className="w-64 font-mono"
              placeholder="python3 -i"
              value={command}
              disabled={!!live}
              onChange={(e) => setCommand(e.target.value)}
            />
          </div>
        )}
        <div className="grid gap-1.5">
          <Label htmlFor="term-user">User</Label>
          <Input
            id="term-user"
            className="w-40"
            placeholder="image default"
            value={user}
            disabled={!!live}
            onChange={(e) => setUser(e.target.value)}
          />
        </div>
        {live ? (
          <Button type="button" variant="outline" onClick={() => setSession(null)}>
            <Unplug aria-hidden />
            Disconnect
          </Button>
        ) : (
          <Button type="submit" disabled={!running}>
            <Plug aria-hidden />
            {session ? "Connect again" : "Connect"}
          </Button>
        )}
      </form>
      {formError && (
        <p role="alert" className="text-sm text-destructive">
          {formError}
        </p>
      )}
      <Status running={running} session={session} started={started} ended={ended} />
      {session && (
        <TerminalView
          key={session.n}
          env={env}
          id={id}
          opts={{ cmd: session.cmd, user: session.user, container }}
          onStarted={() => setStarted(true)}
          onEnd={setEnded}
        />
      )}
    </div>
  );
}

function Status({
  running,
  session,
  started,
  ended,
}: {
  running: boolean;
  session: Session | null;
  started: boolean;
  ended: SessionEnd | null;
}) {
  if (ended?.kind === "error") {
    return (
      <p role="alert" className="text-sm text-destructive">
        {ended.message}
      </p>
    );
  }
  const text =
    ended?.kind === "exited"
      ? ended.code === null
        ? "The process exited."
        : `The process exited with code ${ended.code}.`
      : ended?.kind === "lost"
        ? "The connection dropped, which ended the session."
        : !running && !session
          ? "Start the container to open a terminal."
          : !session
            ? "Choose a shell and connect. The session is recorded in the audit log."
            : started
              ? "Connected. Ctrl+Shift+C copies the selection."
              : "Starting…";
  return (
    <p role="status" className="text-sm text-muted-foreground">
      {text}
    </p>
  );
}
