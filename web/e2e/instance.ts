import { spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

/** A FleetOps server of its own, for specs that must not share the suite's (backup and restore). */
export interface Instance {
  url: string;
  data: string;
  stop: () => void;
}

const bin = process.env.FLEETOPS_BIN ?? "../target/release/fleetops";

export async function startInstance(port: number, env: Record<string, string> = {}): Promise<Instance> {
  const data = fs.mkdtempSync(path.join(os.tmpdir(), "fleetops-e2e-"));
  const url = `http://127.0.0.1:${port}`;
  const child: ChildProcess = spawn(bin, ["--data", data, "--bind", `127.0.0.1:${port}`], {
    env: { ...process.env, ...env },
    stdio: "ignore",
  });
  await waitUp(url);
  return {
    url,
    data,
    stop: () => {
      child.kill();
      fs.rmSync(data, { recursive: true, force: true });
    },
  };
}

export async function waitUp(url: string, timeoutMs = 30_000) {
  const start = Date.now();
  for (;;) {
    try {
      if ((await fetch(`${url}/api/v1/health`)).ok) return;
    } catch {
      // Not listening yet.
    }
    if (Date.now() - start > timeoutMs) throw new Error(`${url} did not start`);
    await new Promise((r) => setTimeout(r, 250));
  }
}
