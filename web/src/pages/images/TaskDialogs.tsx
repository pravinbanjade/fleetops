import { useMutation } from "@tanstack/react-query";
import { useEffect, useRef, useState, type CSSProperties, type FormEvent, type ReactNode } from "react";
import { buildImage, pullImage } from "@/api/gen/sdk.gen";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useTask, useTaskTracker } from "@/live/TaskProvider";
import type { TrackedTask } from "@/live/tasks";
import { errorMessage } from "@/lib/errors";
import { formatBytes } from "@/lib/format";
import { buildLog, pullProgress } from "@/lib/progress";
import { FormError } from "@/pages/AuthLayout";

/** The server's upload limit for a build context. */
export const BUILD_UPLOAD_MAX = 100 * 1024 * 1024;

export function PullDialog({ env, onClose }: { env: string; onClose: () => void }) {
  const tracker = useTaskTracker();
  const [ref, setRef] = useState("");
  const [taskId, setTaskId] = useState<string | null>(null);
  const task = useTask(taskId);
  const m = useMutation({
    meta: { inline: true },
    mutationFn: () => pullImage({ path: { env }, body: { image: ref.trim() } }),
    onSuccess: ({ data }) => {
      tracker.track(data.task_id, env, `Pull ${ref.trim()}`);
      setTaskId(data.task_id);
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    m.mutate();
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Pull image</DialogTitle>
          <DialogDescription>From Docker Hub or any registry the engine can reach without logging in.</DialogDescription>
        </DialogHeader>
        {task ? (
          <TaskView task={task} onClose={onClose}>
            <PullView messages={task.messages} />
          </TaskView>
        ) : (
          <form onSubmit={submit} className="grid gap-4">
            <div className="grid gap-2">
              <Label htmlFor="pull-ref">Image</Label>
              <Input
                id="pull-ref"
                required
                placeholder="nginx:1.27 or ghcr.io/org/app:2"
                value={ref}
                onChange={(e) => setRef(e.target.value)}
              />
            </div>
            <FormError message={m.error ? errorMessage(m.error) : null} />
            <DialogFooter>
              <Button type="submit" disabled={m.isPending}>
                Pull
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

const isArchive = (name: string) => /\.(tar|tar\.gz|tgz)$/i.test(name);

export function BuildDialog({ env, onClose }: { env: string; onClose: () => void }) {
  const tracker = useTaskTracker();
  const [file, setFile] = useState<File | null>(null);
  const [tag, setTag] = useState("");
  const [path, setPath] = useState("");
  const [invalid, setInvalid] = useState<string | null>(null);
  const [taskId, setTaskId] = useState<string | null>(null);
  const task = useTask(taskId);
  const archive = !!file && isArchive(file.name);
  const m = useMutation({
    meta: { inline: true },
    mutationFn: (f: File) =>
      buildImage({
        path: { env },
        body: archive
          ? { context: f, tag: tag.trim() || null, dockerfile_path: path.trim() || null }
          : { dockerfile: f, tag: tag.trim() || null },
      }),
    onSuccess: ({ data }) => {
      tracker.track(data.task_id, env, `Build ${tag.trim() || file!.name}`);
      setTaskId(data.task_id);
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!file) return setInvalid("Choose a Dockerfile or a tar of the build context.");
    if (file.size > BUILD_UPLOAD_MAX) {
      return setInvalid(`${file.name} is ${formatBytes(file.size)}; the limit is ${formatBytes(BUILD_UPLOAD_MAX)}.`);
    }
    setInvalid(null);
    m.mutate(file);
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Build image</DialogTitle>
          <DialogDescription>
            Upload a lone Dockerfile, or a .tar / .tar.gz of the whole build context.
          </DialogDescription>
        </DialogHeader>
        {task ? (
          <TaskView task={task} onClose={onClose}>
            <LogView lines={buildLog(task.messages)} />
          </TaskView>
        ) : (
          <form onSubmit={submit} className="grid gap-4">
            <div className="grid gap-2">
              <Label htmlFor="build-file">Dockerfile or context</Label>
              <Input id="build-file" type="file" onChange={(e) => setFile(e.target.files?.[0] ?? null)} />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="build-tag">Tag</Label>
              <Input id="build-tag" placeholder="app:1.2" value={tag} onChange={(e) => setTag(e.target.value)} />
            </div>
            {archive && (
              <div className="grid gap-2">
                <Label htmlFor="build-path">Dockerfile path in the context</Label>
                <Input id="build-path" placeholder="Dockerfile" value={path} onChange={(e) => setPath(e.target.value)} />
              </div>
            )}
            <FormError message={invalid ?? (m.error ? errorMessage(m.error) : null)} />
            <DialogFooter>
              <Button type="submit" disabled={m.isPending}>
                Build
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

function TaskView({ task, onClose, children }: { task: TrackedTask; onClose: () => void; children: ReactNode }) {
  return (
    <div className="grid gap-4">
      {children}
      <p role="status" className="text-sm">
        {task.status === "running" ? (
          <span className="text-muted-foreground">Running… You can close this; you will be told when it finishes.</span>
        ) : task.status === "succeeded" ? (
          "Done."
        ) : (
          <span className="text-destructive">Failed: {task.error}</span>
        )}
      </p>
      <DialogFooter>
        <Button variant="outline" onClick={onClose}>
          Close
        </Button>
      </DialogFooter>
    </div>
  );
}

function PullView({ messages }: { messages: unknown[] }) {
  const { layers, lines } = pullProgress(messages);
  return (
    <div className="grid gap-3">
      {layers.length > 0 && (
        <ul aria-label="Layers" className="grid gap-1.5 text-xs">
          {layers.map((l) => (
            <li key={l.id} className="grid grid-cols-[7rem_1fr_9rem] items-center gap-2">
              <code>{l.id}</code>
              <Bar
                label={`Layer ${l.id}`}
                percent={
                  l.total
                    ? Math.round(((l.current ?? 0) / l.total) * 100)
                    : l.status.endsWith("complete") || l.status === "Already exists"
                      ? 100
                      : 0
                }
              />
              <span className="truncate text-muted-foreground">
                {l.status}
                {l.total && l.status === "Downloading" ? ` ${formatBytes(l.current)} / ${formatBytes(l.total)}` : ""}
              </span>
            </li>
          ))}
        </ul>
      )}
      <LogView lines={lines} />
    </div>
  );
}

function Bar({ label, percent }: { label: string; percent: number }) {
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent}
      className="h-1.5 overflow-hidden rounded-full bg-muted"
    >
      {/* Progress ticks often: a shorter glide keeps it close to the bytes. */}
      <div className="bar-fill bg-primary [--bar-ms:300ms]" style={{ "--share": percent / 100 } as CSSProperties} />
    </div>
  );
}

/** Output lines, kept scrolled to the end as they arrive. */
function LogView({ lines }: { lines: string[] }) {
  const ref = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
  }, [lines.length]);
  if (lines.length === 0) return null;
  return (
    <pre
      ref={ref}
      aria-label="Output"
      className="max-h-80 overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs whitespace-pre-wrap"
    >
      {lines.join("\n")}
    </pre>
  );
}
