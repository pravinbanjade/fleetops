import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { DestinationView } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";

const SLACK: DestinationView = {
  id: 1,
  name: "on-call",
  kind: "slack",
  host: "hooks.slack.com",
  recipients: [],
  secret_set: true,
  signing_secret_set: false,
  last_error: null,
  last_error_at: null,
};

const page = (extra: Record<string, unknown> = {}, licensed = true) => ({
  ...loggedIn,
  "GET /api/v1/edition": { json: licensed ? { edition: "enterprise", features: ["notifications"] } : { edition: "community", features: [] } },
  "GET /api/v1/ee/notifications/destinations": { json: [SLACK] },
  "GET /api/v1/ee/notifications/rules": { json: [] },
  "GET /api/v1/ee/notifications/smtp": { json: null },
  "GET /api/v1/ee/notifications/deliveries": { json: [] },
  ...extra,
});

describe("notifications page", () => {
  it("creates a Slack destination without echoing the URL", async () => {
    const url = "https://hooks.slack.com/services/T/B/SECRET";
    const calls = mockFetch({ ...page(), "POST /api/v1/ee/notifications/destinations": { json: { ...SLACK, id: 2, name: "sec" } } });
    renderApp("/admin/notifications");
    await userEvent.click(await screen.findByRole("button", { name: "Add destination" }));
    const d = await screen.findByRole("dialog", { name: "Add destination" });
    await userEvent.type(within(d).getByLabelText("Name"), "sec");
    await userEvent.type(within(d).getByLabelText("Webhook URL"), url);
    await userEvent.click(within(d).getByRole("button", { name: "Add" }));
    await waitFor(() => expect(calls.find((c) => c.method === "POST")?.body).toMatchObject({ name: "sec", kind: "slack", url }));
    expect(screen.queryByText(url)).toBeNull();
    expect(screen.getAllByText("hooks.slack.com").length).toBeGreaterThan(0);
  });

  it("needs a destination before a rule can be saved", async () => {
    mockFetch(page());
    renderApp("/admin/notifications");
    await userEvent.click(await screen.findByRole("button", { name: "Add rule" }));
    const d = await screen.findByRole("dialog", { name: "Add rule" });
    await userEvent.type(within(d).getByLabelText("Name"), "prod");
    expect(within(d).getByRole("button", { name: "Add" })).toBeDisabled();
    await userEvent.click(within(d).getByRole("checkbox", { name: "on-call" }));
    expect(within(d).getByRole("button", { name: "Add" })).toBeEnabled();
  });

  it("shows a failing destination's error", async () => {
    mockFetch(page({ "GET /api/v1/ee/notifications/destinations": { json: [{ ...SLACK, last_error: "answered 404 (hooks.slack.com): no_team", last_error_at: 1 }] } }));
    renderApp("/admin/notifications");
    expect(await screen.findByText(/Failing: answered 404/)).toBeInTheDocument();
  });

  it("reports a failed test", async () => {
    mockFetch({ ...page(), "POST /api/v1/ee/notifications/destinations/1/test": () => json(502, { code: "upstream", message: "answered 404 (hooks.slack.com): no_team" }) });
    renderApp("/admin/notifications");
    await userEvent.click(await screen.findByRole("button", { name: "Send test to on-call" }));
    expect(await screen.findByText(/no_team/)).toBeInTheDocument();
  });

  it("keeps the SMTP password write-only", async () => {
    mockFetch(
      page({
        "GET /api/v1/ee/notifications/smtp": {
          json: { host: "smtp.example.com", port: 587, security: "starttls", username: "u", password_set: true, from_address: "f@example.com" },
        },
      }),
    );
    renderApp("/admin/notifications");
    const pw = await screen.findByLabelText("SMTP password");
    expect(pw).toHaveValue("");
    expect(screen.getByText("Saved")).toBeInTheDocument();
  });

  it("is frozen without a license but still allows delete", async () => {
    mockFetch(page({}, false));
    renderApp("/admin/notifications");
    expect(await screen.findByText(/Notifications need a license/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add destination" })).toBeNull();
    expect(await screen.findByRole("button", { name: "Delete on-call" })).toBeInTheDocument();
  });
});
