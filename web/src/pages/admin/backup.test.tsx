import { waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { StagedBackup } from "@/api/gen/types.gen";
import { isEnterpriseBuild } from "@/ee-slots";
import { mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";

const STAGED: StagedBackup = {
  id: "a".repeat(32),
  summary: {
    fleetops_version: "0.2.0",
    created_at: 1_790_000_000,
    edition: "enterprise",
    source: null,
    counts: { users: 3, environments: 2, audit_log: 40 },
    has_enterprise_data: true,
  },
};

async function openBackup(screen: typeof import("@testing-library/react").screen) {
  const user = userEvent.setup();
  const file = new File(["x"], "b.fob");
  await user.upload(await screen.findByLabelText("Backup file"), file);
  await user.type(screen.getByLabelText("Passphrase"), "correct horse battery");
  await user.click(screen.getByRole("button", { name: "Open backup" }));
  return user;
}

describe("backup page", () => {
  it("restore requires typing RESTORE", async () => {
    const calls = mockFetch({
      ...loggedIn,
      "POST /api/v1/backup/stage": { json: STAGED },
      "POST /api/v1/backup/stage/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/restore": { status: 202 },
    });
    const { screen } = renderApp("/admin/backup");
    const user = await openBackup(screen);
    const restore = await screen.findByRole("button", { name: "Restore" });
    expect(screen.getByText("3")).toBeInTheDocument();
    expect(restore).toBeDisabled();
    await user.type(screen.getByLabelText("Type RESTORE to confirm"), "restore");
    expect(restore).toBeDisabled();
    await user.clear(screen.getByLabelText("Type RESTORE to confirm"));
    await user.type(screen.getByLabelText("Type RESTORE to confirm"), "RESTORE");
    expect(restore).toBeEnabled();
    await user.click(restore);
    await waitFor(() => expect(calls.some((c) => c.path.endsWith("/restore"))).toBe(true));
    const sent = calls.find((c) => c.path.endsWith("/restore"))!;
    expect(sent.body).toEqual({ confirm: "RESTORE" });
  });

  it("download rejects mismatched passphrases", async () => {
    const calls = mockFetch(loggedIn);
    const { screen } = renderApp("/admin/backup");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Download backup" }));
    await user.type(screen.getByLabelText("Passphrase"), "correct horse battery");
    await user.type(screen.getByLabelText("Passphrase again"), "correct horse batterz");
    await user.click(screen.getByRole("button", { name: "Download" }));
    expect(await screen.findByText("The passphrases do not match.")).toBeInTheDocument();
    expect(calls.some((c) => c.path === "/api/v1/backup")).toBe(false);
  });

  it("summary warns about enterprise data on community", async () => {
    mockFetch({ ...loggedIn, "POST /api/v1/backup/stage": { json: STAGED } });
    const { screen } = renderApp("/admin/backup");
    await openBackup(screen);
    await screen.findByRole("button", { name: "Restore" });
    const warning = screen.queryByText(/Community keeps it but doesn't use it/);
    if (isEnterpriseBuild) expect(warning).toBeNull();
    else expect(warning).toBeInTheDocument();
  });
});

describe("first-run setup", () => {
  it("offers restore from backup, with no confirm word", async () => {
    const calls = mockFetch({
      "GET /api/v1/auth/status": { json: { setup_required: true, setup_open: true } },
      "POST /api/v1/auth/setup/backup": { json: STAGED },
    });
    const { screen } = renderApp("/setup");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Restore from backup" }));
    await openBackup(screen);
    expect(await screen.findByRole("button", { name: "Restore" })).toBeEnabled();
    expect(screen.queryByLabelText("Type RESTORE to confirm")).toBeNull();
    expect(calls.some((c) => c.path === "/api/v1/auth/setup/backup")).toBe(true);
  });
});
