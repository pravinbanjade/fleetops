import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { AuditEntry } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { AUDIT_PAGE } from "@/api/queries";

const entry = (id: number, over: Partial<AuditEntry> = {}): AuditEntry => ({
  id,
  ts: 1_790_000_000 + id,
  action: "container.stop",
  result: "success",
  user_id: 1,
  username: "admin",
  environment_id: 1,
  target_type: "container",
  target_id: `c${id}`,
  ...over,
});

const rows = (screen: typeof import("@testing-library/react").screen) =>
  within(screen.getByRole("table", { name: "Audit log" })).getAllByRole("row").slice(1);

describe("audit log", () => {
  it("lists entries with environment names, failures and details", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/audit": {
        json: [
          entry(2, { action: "exec", result: "failure", detail: { cmd: ["sh"] } }),
          entry(1, { environment_id: null, action: "user.create", target_type: "user", target_id: "2" }),
        ],
      },
    });
    const { screen } = renderApp("/admin/audit");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    const [exec, create] = rows(screen);
    expect(exec).toHaveTextContent("local");
    expect(within(exec).getByText("failure")).toBeInTheDocument();
    await userEvent.click(within(exec).getByText("show"));
    expect(exec).toHaveTextContent('"sh"');
    expect(create).toHaveTextContent("user 2");
    expect(screen.queryByRole("button", { name: "Load older entries" })).not.toBeInTheDocument();
  });

  it("loads older entries a page at a time", async () => {
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/audit": (c) => {
        const before = new URL(c.path, "http://x").searchParams.get("before");
        const top = before ? Number(before) - 1 : 150;
        const n = Math.min(AUDIT_PAGE, top);
        return json(200, Array.from({ length: n }, (_, i) => entry(top - i)));
      },
    });
    const { screen } = renderApp("/admin/audit");
    await waitFor(() => expect(rows(screen)).toHaveLength(AUDIT_PAGE));
    await userEvent.click(screen.getByRole("button", { name: "Load older entries" }));
    await waitFor(() => expect(rows(screen)).toHaveLength(150));
    expect(calls.filter((c) => c.path.startsWith("/api/v1/audit")).map((c) => c.path)).toEqual([
      "/api/v1/audit?limit=100",
      "/api/v1/audit?limit=100&before=51",
    ]);
    expect(screen.queryByRole("button", { name: "Load older entries" })).not.toBeInTheDocument();
  });
});
