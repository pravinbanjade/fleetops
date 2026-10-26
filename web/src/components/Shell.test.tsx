import { act, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { ADMIN, loggedIn, renderApp } from "@/test/app";

describe("shell", () => {
  it("logs out and returns to the login page", async () => {
    let out = false;
    // Once logged out, the shell's own queries are refused too.
    const orGone = (body: unknown) => () => (out ? json(401, { code: "unauthorized", message: "no session" }) : json(200, body));
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/me": orGone(ADMIN),
      "GET /api/v1/envs": orGone(loggedIn["GET /api/v1/envs"].json),
      "GET /api/v1/envs/local/status": orGone(loggedIn["GET /api/v1/envs/local/status"].json),
      "POST /api/v1/auth/logout": () => ((out = true), new Response(null, { status: 204 })),
    });
    const { history, screen } = renderApp("/envs/local");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "admin" }));
    await user.click(await screen.findByRole("menuitem", { name: "Log out" }));
    expect(await screen.findByText("Log in to FleetOps")).toBeInTheDocument();
    expect(history.location.pathname).toBe("/login");
    expect(calls.find((c) => c.path === "/api/v1/auth/logout")?.headers.get("x-csrf-token")).toBe("csrf-1");
  });

  it("has no theme choice in the user menu: the app is dark only", async () => {
    mockFetch(loggedIn);
    const { screen } = renderApp("/envs/local");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "admin" }));
    expect(await screen.findByRole("menuitem", { name: "Log out" })).toBeInTheDocument();
    expect(screen.queryByText("Theme")).toBeNull();
    expect(screen.queryByRole("menuitemradio")).toBeNull();
  });

  it("warns while the live connection is down and refetches after it returns", async () => {
    const calls = mockFetch(loggedIn);
    const { screen } = renderApp("/envs/local");
    await screen.findByRole("heading", { name: "local" });
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    act(() => FakeWebSocket.latest().open());
    act(() => FakeWebSocket.latest().drop());
    expect(await screen.findByText(/Lost the live connection/)).toBeInTheDocument();
    const status = () => calls.filter((c) => c.path === "/api/v1/envs/local/status").length;
    const before = status();
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(2), { timeout: 2000 });
    act(() => FakeWebSocket.latest().open());
    await waitFor(() => expect(screen.queryByText(/Lost the live connection/)).toBeNull());
    await waitFor(() => expect(status()).toBeGreaterThan(before));
  });

  it("sends the user to login when the live connection reports the session ended", async () => {
    let out = false;
    mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/me": () => (out ? json(401, { code: "unauthorized", message: "no session" }) : json(200, ADMIN)),
    });
    const { history, screen } = renderApp("/envs/local");
    await screen.findByRole("heading", { name: "local" });
    await waitFor(() => expect(FakeWebSocket.instances.length).toBeGreaterThan(0));
    const sock = FakeWebSocket.latest();
    act(() => sock.open());
    out = true;
    act(() => {
      sock.receive({ sub: null, type: "error", code: "unauthorized", message: "session expired" });
      sock.drop();
    });
    await waitFor(() => expect(history.location.pathname).toBe("/login"));
    expect(history.location.search).toContain("redirect=%2Fenvs%2Flocal");
  });

  it("checks the session when reconnecting fails, and sends the user to login if it ended", async () => {
    let out = false;
    mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/me": () => (out ? json(401, { code: "unauthorized", message: "no session" }) : json(200, ADMIN)),
    });
    const { history, screen } = renderApp("/envs/local");
    await screen.findByRole("heading", { name: "local" });
    await waitFor(() => expect(FakeWebSocket.instances.length).toBeGreaterThan(0));
    act(() => FakeWebSocket.latest().open());
    out = true;
    act(() => FakeWebSocket.latest().drop());
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(2), { timeout: 2000 });
    act(() => FakeWebSocket.latest().drop()); // the server refused the upgrade
    await waitFor(() => expect(history.location.pathname).toBe("/login"));
  });
});
