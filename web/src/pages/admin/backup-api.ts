import { apiRequest } from "@/api/client";
import type { StagedBackup } from "@/api/gen/types.gen";

/** Matches the server's minimum. */
export const MIN_PASSPHRASE = 12;

/** Asks for a backup and saves it through the browser. */
export async function downloadBackup(passphrase: string) {
  const res = await apiRequest("/api/v1/backup", { method: "POST", json: { passphrase } });
  const name = /filename="([^"]+)"/.exec(res.headers.get("content-disposition") ?? "")?.[1] ?? "fleetops-backup.fob";
  const url = URL.createObjectURL(await res.blob());
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** Uploads a backup; `setup` is the first-run screen's unauthenticated route. */
export async function stageBackup(file: File, passphrase: string, setup: boolean): Promise<StagedBackup> {
  const form = new FormData();
  form.append("file", file);
  form.append("passphrase", passphrase);
  const res = await apiRequest(setup ? "/api/v1/auth/setup/backup" : "/api/v1/backup/stage", { method: "POST", body: form });
  return (await res.json()) as StagedBackup;
}

export async function restoreStaged(id: string, setup: boolean) {
  await apiRequest(setup ? `/api/v1/auth/setup/backup/${id}/restore` : `/api/v1/backup/stage/${id}/restore`, {
    method: "POST",
    json: setup ? undefined : { confirm: "RESTORE" },
  });
}

export async function discardStaged(id: string) {
  await apiRequest(`/api/v1/backup/stage/${id}`, { method: "DELETE" });
}

/** Resolves once the server went away (or 3 s passed) and answers again. */
export async function waitForRestart(pollMs = 1000) {
  const up = async () => {
    try {
      return (await fetch(new Request(new URL("/api/v1/health", window.location.origin)))).ok;
    } catch {
      return false;
    }
  };
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
  const start = Date.now();
  while (Date.now() - start < 3000 && (await up())) await sleep(pollMs / 4);
  while (!(await up())) await sleep(pollMs);
}
