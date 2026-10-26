import { useState } from "react";
import { LogView } from "@/pages/containers/logs/LogView";
import { DEFAULT_QUERY, useLogStream } from "@/pages/containers/logs/useLogStream";

/**
 * The newest lines of a container's log, followed live: the panel's view of
 * what the container is saying right now. Scrolling up pauses following.
 */
export function LogTail({ env, id, startedAt, lines = 200 }: { env: string; id: string; startedAt?: string; lines?: number }) {
  const { buffer, version, state } = useLogStream(env, id, { ...DEFAULT_QUERY, tail: lines }, startedAt);
  const [follow, setFollow] = useState(true);
  return (
    <div className="relative flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg bg-[#0a0a0a] text-[12.5px]">
      {buffer.length === 0 ? (
        <p className="p-3 text-meta">{state.state === "error" ? state.message : state.state === "ended" ? "The log has ended." : "Waiting for log lines…"}</p>
      ) : (
        <LogView buffer={buffer} version={version} timestamps wrap follow={follow} onFollowChange={setFollow} />
      )}
    </div>
  );
}
