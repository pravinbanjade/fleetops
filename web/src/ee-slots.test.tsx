import { waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { isEnterpriseBuild } from "@/ee-slots";
import { mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";

describe.runIf(!isEnterpriseBuild)("community build", () => {
  it("never asks for the edition and shows no Enterprise UI", async () => {
    const calls = mockFetch({ ...loggedIn, "GET /api/v1/audit": { json: [] } });
    const { screen } = renderApp("/admin/audit");
    await screen.findByRole("table", { name: "Audit log" });
    await waitFor(() => expect(screen.getByRole("navigation", { name: "Administration" })).toBeInTheDocument());
    expect(screen.queryByRole("link", { name: "License" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Export" })).not.toBeInTheDocument();
    expect(calls.some((c) => c.path.startsWith("/api/v1/edition") || c.path.startsWith("/api/v1/ee/"))).toBe(false);
    expect(document.querySelector("[data-edition]")?.getAttribute("data-edition")).toBe("ce");
  });
});
