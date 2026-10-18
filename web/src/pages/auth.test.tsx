import { waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { ADMIN, loggedIn, renderApp } from "@/test/app";

const fresh = { "GET /api/v1/auth/status": { json: { setup_required: true, setup_open: true } } };

describe("setup page", () => {
  it("creates the administrator and opens the app", async () => {
    let created = false;
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/status": () => json(200, { setup_required: !created, setup_open: !created }),
      "POST /api/v1/auth/setup": () => ((created = true), json(201, ADMIN)),
    });
    const { history, screen } = renderApp("/setup");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Password"), "correct-horse-9");
    await user.type(screen.getByLabelText("Confirm password"), "correct-horse-9");
    await user.click(screen.getByRole("button", { name: "Create administrator" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local"));
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ username: "admin", password: "correct-horse-9" });
  });

  it("refuses mismatched passwords without calling the server", async () => {
    const calls = mockFetch(fresh);
    const { screen } = renderApp("/setup");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Password"), "correct-horse-9");
    await user.type(screen.getByLabelText("Confirm password"), "correct-horse-8");
    await user.click(screen.getByRole("button", { name: "Create administrator" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The passwords do not match.");
    expect(calls.some((c) => c.method === "POST")).toBe(false);
  });

  it("shows the server's validation message", async () => {
    mockFetch({
      ...fresh,
      "POST /api/v1/auth/setup": { status: 400, json: { code: "invalid", message: "password must be 8-1024 characters" } },
    });
    const { screen } = renderApp("/setup");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Password"), "short");
    await user.type(screen.getByLabelText("Confirm password"), "short");
    await user.click(screen.getByRole("button", { name: "Create administrator" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("password must be 8-1024 characters");
  });

  it("explains a locked setup window", async () => {
    mockFetch({ "GET /api/v1/auth/status": { json: { setup_required: true, setup_open: false } } });
    const { screen } = renderApp("/setup");
    expect(await screen.findByText("Setup is locked")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create administrator" })).toBeNull();
  });
});

describe("login page", () => {
  const out = { ...loggedIn, "GET /api/v1/auth/me": { status: 401, json: { code: "unauthorized", message: "no" } } };

  it("logs in and returns to the page the user asked for", async () => {
    mockFetch({ ...out, "POST /api/v1/auth/login": { json: ADMIN } });
    const { history, screen } = renderApp("/login?redirect=%2Felsewhere");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Username"), "admin");
    await user.type(screen.getByLabelText("Password"), "correct-horse-9");
    await user.click(screen.getByRole("button", { name: "Log in" }));
    await waitFor(() => expect(history.location.pathname).toBe("/elsewhere"));
  });

  it("shows the server's message for bad credentials and rate limiting", async () => {
    let n = 0;
    mockFetch({
      ...out,
      "POST /api/v1/auth/login": () =>
        ++n === 1
          ? json(401, { code: "unauthorized", message: "invalid username or password" })
          : json(429, { code: "rate_limited", message: "too many failed logins; try again in 5 minutes" }),
    });
    const { screen } = renderApp("/login");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Username"), "admin");
    await user.type(screen.getByLabelText("Password"), "wrong-password");
    await user.click(screen.getByRole("button", { name: "Log in" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("invalid username or password");
    await user.click(screen.getByRole("button", { name: "Log in" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("try again in 5 minutes"));
  });

  it("ignores a redirect to another site", async () => {
    mockFetch({ ...out, "POST /api/v1/auth/login": { json: ADMIN } });
    const { history, screen } = renderApp("/login?redirect=%2F%2Fevil.example%2F");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Username"), "admin");
    await user.type(screen.getByLabelText("Password"), "correct-horse-9");
    await user.click(screen.getByRole("button", { name: "Log in" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local"));
  });

  it("offers single sign-on and keeps the password form one click away", async () => {
    mockFetch({ ...out, "GET /api/v1/auth/providers": { json: [{ id: 3, name: "Acme Okta", kind: "oidc" }] } });
    const { screen } = renderApp("/login?redirect=%2Fenvs");
    const link = await screen.findByRole("link", { name: "Sign in with Acme Okta" });
    expect(link).toHaveAttribute("href", "/api/v1/ee/sso/3/start?return_to=%2Fenvs");
    expect(screen.queryByLabelText("Password")).toBeNull();
    await userEvent.setup().click(screen.getByRole("button", { name: "Sign in with password" }));
    expect(await screen.findByLabelText("Password")).toBeInTheDocument();
  });

  it("explains why single sign-on sent the user back", async () => {
    mockFetch(out);
    const { screen } = renderApp("/login?sso_error=seat_limit");
    expect(await screen.findByRole("alert")).toHaveTextContent("FleetOps has reached its licensed user limit. Ask an admin.");
  });

  it("ignores an unknown sso_error", async () => {
    mockFetch(out);
    const { screen } = renderApp("/login?sso_error=%3Cscript%3E");
    await screen.findByLabelText("Password");
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
