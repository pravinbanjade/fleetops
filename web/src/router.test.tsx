import { within, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { safeRedirect } from "./router";
import { json, mockFetch } from "@/test/fetch";
import { ADMIN, loggedIn, renderApp } from "@/test/app";

const unauthorized = { status: 401, json: { code: "unauthorized", message: "not logged in" } };

describe("routing", () => {
  it("sends every page to setup while no user exists", async () => {
    mockFetch({ "GET /api/v1/auth/status": { json: { setup_required: true, setup_open: true } } });
    const { history, screen } = renderApp("/envs/local");
    expect(await screen.findByText("Welcome to FleetOps")).toBeInTheDocument();
    expect(history.location.pathname).toBe("/setup");
  });

  it("sends visitors without a session to login, remembering where they were going", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/auth/me": unauthorized });
    const { history, screen } = renderApp("/envs/local");
    expect(await screen.findByText("Log in to FleetOps")).toBeInTheDocument();
    expect(history.location.pathname).toBe("/login");
    expect(history.location.search).toContain("redirect=%2Fenvs%2Flocal");
  });

  it("opens the first environment from the start page", async () => {
    mockFetch(loggedIn);
    const { history, screen } = renderApp("/");
    expect(await screen.findByRole("heading", { name: "local" })).toBeInTheDocument();
    expect(history.location.pathname).toBe("/envs/local");
  });

  it("explains an instance with no environments", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs": { json: [] } });
    const { screen } = renderApp("/");
    expect(await screen.findByText("No environments")).toBeInTheDocument();
  });

  it("tells a user without access to ask an admin", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/me": { json: { ...ADMIN, user: { ...ADMIN.user, username: "bob", role: "user" } } },
      "GET /api/v1/envs": { json: [] },
    });
    const { screen } = renderApp("/");
    expect(await screen.findByText("You don't have access to any environments yet. Ask an admin.")).toBeInTheDocument();
  });

  it("sends a logged-in user away from login and setup", async () => {
    mockFetch(loggedIn);
    const { history } = renderApp("/login");
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local"));
  });

  it("shows not found for unknown paths", async () => {
    mockFetch(loggedIn);
    const { screen } = renderApp("/nope/nothing");
    expect(await screen.findByText("Page not found")).toBeInTheDocument();
  });

  it("sends the user to login when the session is lost mid-use", async () => {
    let lost = false;
    mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/me": () => (lost ? json(401, unauthorized.json) : json(200, ADMIN)),
      "GET /api/v1/auth/sessions": () => (lost ? json(401, unauthorized.json) : json(200, [])),
    });
    const { history, screen } = renderApp("/envs/local");
    await screen.findByRole("heading", { name: "local" });
    lost = true;
    await userEvent.click(screen.getByRole("button", { name: "admin" }));
    await userEvent.click(await screen.findByRole("menuitem", { name: "Account" }));
    await waitFor(() => expect(history.location.pathname).toBe("/login"));
    expect(history.location.search).toContain("redirect=%2Faccount");
  });
});

describe("a lost session", () => {
  it("does not show the previous user's data to the next one", async () => {
    let lost = false;
    let sessionsCalls = 0;
    const BOB = { csrf_token: "csrf-2", user: { ...ADMIN.user, id: 2, username: "bob" } };
    mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/me": () => (lost ? json(401, unauthorized.json) : json(200, ADMIN)),
      "GET /api/v1/envs/local/info": () => (lost ? json(401, unauthorized.json) : json(200, {})),
      "POST /api/v1/auth/login": () => ((lost = false), json(200, BOB)),
      "GET /api/v1/auth/sessions": () =>
        ++sessionsCalls === 1
          ? json(200, [{ id: 7, created_at: 0, last_seen_at: 0, expires_at: 0, current: false, user_agent: "AdminsLaptop", ip: "10.0.0.9" }])
          : new Promise<Response>(() => {}), // bob's list never arrives: only cached data could show
    });
    const { history, screen } = renderApp("/account");
    const user = userEvent.setup();
    expect(await screen.findByText("AdminsLaptop")).toBeInTheDocument();
    lost = true;
    await user.click(within(screen.getByRole("region", { name: "Environments" })).getByRole("link", { name: /^local/ }));
    await waitFor(() => expect(history.location.pathname).toBe("/login"));
    await user.type(screen.getByLabelText("Username"), "bob");
    await user.type(screen.getByLabelText("Password"), "bobs-password-1");
    await user.click(screen.getByRole("button", { name: "Log in" }));
    await user.click(await screen.findByRole("button", { name: "bob" }));
    await user.click(await screen.findByRole("menuitem", { name: "Account" }));
    expect(await screen.findByRole("heading", { name: "Account" })).toBeInTheDocument();
    expect(screen.queryByText("AdminsLaptop")).toBeNull();
  });
});

describe("safeRedirect", () => {
  it("keeps local paths only", () => {
    expect(safeRedirect("/envs/local?x=1")).toBe("/envs/local?x=1");
    expect(safeRedirect("//evil.example/")).toBeUndefined();
    expect(safeRedirect("https://evil.example/")).toBeUndefined();
    expect(safeRedirect(42)).toBeUndefined();
  });
});
