import { waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { backupBanner, localTimeOf, type BackupStatus } from "./backup";

const status = (over: Partial<BackupStatus> = {}): BackupStatus => ({
  licensed: true,
  enabled: true,
  paused: false,
  last_run: null,
  last_failed: false,
  next_run_at: null,
  ...over,
});

const LICENSE = {
  status: "valid", source: "uploaded", reason: null, id: "l", customer: "Acme", expires_at: 4_000_000_000,
  max_users: null, features: ["backup"], users_in_use: 1, max_nodes: null, nodes_in_use: 0,
};

const page = (st: BackupStatus, extra: Record<string, unknown> = {}) => ({
  ...loggedIn,
  "GET /api/v1/edition": { json: { edition: "enterprise", features: st.licensed ? ["backup"] : [] } },
  "GET /api/v1/ee/license": { json: LICENSE },
  "GET /api/v1/ee/backup/status": { json: st },
  "GET /api/v1/ee/backup/schedule": { json: { enabled: true, hour_utc: 2, retention: { kind: "count", value: 7 }, passphrase_set: true } },
  "GET /api/v1/ee/backup/targets": { json: [{ kind: "local", settings: { kind: "local", path: "/mnt/b" }, secret_set: false }] },
  "GET /api/v1/ee/backup/runs": { json: [] },
  "GET /api/v1/ee/backup/recent": {
    json: { backups: [{ target_kind: "local", name: "fleetops-backup-20270115-020000.fob", size: 2048, created_at: 1_800_000_000 }], errors: [] },
  },
  ...extra,
});

describe("backup banner", () => {
  it("paused banner text", () => {
    expect(backupBanner(status({ paused: true, last_failed: true }))).toBe("Scheduled backups paused: license required");
    expect(backupBanner(status())).toBeNull();
  });

  it("failed banner links to backup", async () => {
    mockFetch(page(status({ last_failed: true })));
    const { screen } = renderApp("/envs/local");
    const banner = await screen.findByText(/The last scheduled backup failed/);
    expect(banner.parentElement?.querySelector("a")).toHaveAttribute("href", "/admin/backup");
  });
});

describe("schedule", () => {
  it("hour is shown in local time", () => {
    expect(localTimeOf(2, 0)).toBe("02:00");
    expect(localTimeOf(2, 330)).toBe("07:30");
    expect(localTimeOf(2, -300)).toBe("21:00");
    expect(localTimeOf(23, 120)).toBe("01:00");
  });

  it("recent restore asks for passphrase on mismatch", async () => {
    let calls = 0;
    const sent: unknown[] = [];
    mockFetch(
      page(status(), {
        "POST /api/v1/ee/backup/recent/restore": (c: { body: unknown }) => {
          sent.push(c.body);
          calls += 1;
          return calls === 1
            ? json(400, { code: "invalid", message: "invalid input: this backup was made with a different passphrase; enter it to restore" })
            : json(200, { id: "x".repeat(32), summary: { fleetops_version: "0.2.0", created_at: 1, edition: "enterprise", source: null, counts: {}, has_enterprise_data: true } });
        },
      }),
    );
    const { screen } = renderApp("/admin/backup");
    const user = userEvent.setup();
    const restore = (await screen.findAllByRole("button", { name: "Restore" }))[0];
    await user.click(restore);
    const field = await screen.findByLabelText("This backup's passphrase");
    await user.type(field, "the old passphrase");
    await user.click(screen.getAllByRole("button", { name: "Restore" })[0]);
    await waitFor(() => expect(screen.getByLabelText("Type RESTORE to confirm")).toBeInTheDocument());
    expect(sent[1]).toMatchObject({ passphrase: "the old passphrase" });
  });

  it("unlicensed shows only disable and remove", async () => {
    mockFetch(page(status({ licensed: false, paused: true })));
    const { screen } = renderApp("/admin/backup");
    expect(await screen.findByText(/Scheduled backups need a license/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Turn off scheduled backups" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Run now" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Save schedule" })).toBeNull();
  });
});
