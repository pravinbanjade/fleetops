import { waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { safeRedirect } from "./router";
import { mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";

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
});

describe("safeRedirect", () => {
  it("keeps local paths only", () => {
    expect(safeRedirect("/envs/local?x=1")).toBe("/envs/local?x=1");
    expect(safeRedirect("//evil.example/")).toBeUndefined();
    expect(safeRedirect("https://evil.example/")).toBeUndefined();
    expect(safeRedirect(42)).toBeUndefined();
  });
});
