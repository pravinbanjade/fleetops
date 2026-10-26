import { describe, expect, it } from "vitest";
import { mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";

describe("Tape", () => {
  it("says the environment is unreachable instead of showing stale events", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/status": { json: { state: "down", reason: "socket gone" } } });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByText("Environment unreachable")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Events" })).not.toHaveTextContent("No events yet");
  });
});
