import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";

const sessions = [
  { id: 1, created_at: 0, last_seen_at: 1790000000, expires_at: 0, current: true, user_agent: "Firefox", ip: "10.0.0.2" },
  { id: 2, created_at: 0, last_seen_at: 1790000000, expires_at: 0, current: false, user_agent: "Chrome", ip: "10.0.0.3" },
];

describe("account page", () => {
  it("revokes another session", async () => {
    let list = sessions;
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/sessions": () => json(200, list),
      "DELETE /api/v1/auth/sessions/2": () => ((list = [sessions[0]]), new Response(null, { status: 204 })),
    });
    const { screen } = renderApp("/account");
    const user = userEvent.setup();
    const chrome = (await screen.findByText("Chrome")).closest("li")!;
    await user.click(within(chrome).getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(screen.queryByText("Chrome")).toBeNull());
    expect(calls.find((c) => c.method === "DELETE")?.headers.get("x-csrf-token")).toBe("csrf-1");
    expect(within(screen.getByText("Firefox").closest("li")!).getByRole("button", { name: "Log out" })).toBeInTheDocument();
  });

  it("changes the password", async () => {
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/sessions": { json: sessions },
      "PUT /api/v1/auth/password": { status: 204 },
    });
    const { screen } = renderApp("/account");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Current password"), "old-password-1");
    await user.type(screen.getByLabelText("New password"), "new-password-1");
    await user.type(screen.getByLabelText("Confirm new password"), "new-password-1");
    await user.click(screen.getByRole("button", { name: "Change password" }));
    expect(await screen.findByText("Password changed")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "PUT")?.body).toEqual({
      current_password: "old-password-1",
      new_password: "new-password-1",
    });
    expect(screen.getByLabelText("Current password")).toHaveValue("");
  });

  it("shows a wrong current password inline", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/sessions": { json: sessions },
      "PUT /api/v1/auth/password": { status: 401, json: { code: "unauthorized", message: "current password is wrong" } },
    });
    const { history, screen } = renderApp("/account");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Current password"), "nope-nope-1");
    await user.type(screen.getByLabelText("New password"), "new-password-1");
    await user.type(screen.getByLabelText("Confirm new password"), "new-password-1");
    await user.click(screen.getByRole("button", { name: "Change password" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("current password is wrong");
    expect(history.location.pathname).toBe("/account");
  });
});
