import { act, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { FakeWebSocket } from "@/test/fake-ws";
import { mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";

describe("Tape", () => {
  it("says the environment is unreachable instead of showing stale events", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/status": { json: { state: "down", reason: "socket gone" } } });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByText("Environment unreachable")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Events" })).not.toHaveTextContent("No events yet");
  });

  it("stops saying Live while the connection is down", async () => {
    mockFetch(loggedIn);
    const { screen } = renderApp("/envs/local");
    const tape = await screen.findByRole("region", { name: "Events" });
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    act(() => FakeWebSocket.latest().open());
    await waitFor(() => expect(tape).toHaveTextContent("Live"));
    act(() => FakeWebSocket.latest().drop());
    await waitFor(() => expect(within(tape).queryByText("Live")).toBeNull());
    expect(tape).toHaveTextContent("Reconnecting");
  });
});
