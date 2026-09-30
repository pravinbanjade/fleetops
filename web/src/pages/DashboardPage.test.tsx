import { act, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { loggedIn, renderApp } from "@/test/app";

const info = {
  Containers: 3,
  ContainersRunning: 2,
  ContainersPaused: 0,
  ContainersStopped: 1,
  Images: 5,
  ServerVersion: "29.5.2",
  OperatingSystem: "Ubuntu 24.04",
  Architecture: "aarch64",
  NCPU: 2,
  MemTotal: 2_100_000_000,
};

/** The events subscription the dashboard's shell opened, once the socket is open. */
async function eventsSub() {
  await waitFor(() => expect(FakeWebSocket.instances.length).toBeGreaterThan(0));
  const sock = FakeWebSocket.latest();
  act(() => sock.open());
  const sub = sock.frames().find((f) => f.op === "sub" && f.topic === "events");
  expect(sub).toMatchObject({ env: "local" });
  return { sock, sub: sub.sub as number };
}

describe("dashboard", () => {
  it("shows counts, host details and disk usage", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/info": { json: info },
      "GET /api/v1/envs/local/df": { json: { ImageUsage: { TotalSize: 3_000_000_000, Reclaimable: 1_000_000_000 } } },
      "GET /api/v1/envs/local/volumes": { json: [{ name: "v1" }, { name: "v2" }] },
    });
    const { screen } = renderApp("/envs/local");
    await waitFor(() => expect(screen.getByLabelText("Containers")).toHaveTextContent("3"));
    expect(screen.getByText("2 running · 0 paused · 1 stopped")).toBeInTheDocument();
    expect(screen.getByLabelText("Images")).toHaveTextContent("5");
    await waitFor(() => expect(screen.getByLabelText("Volumes")).toHaveTextContent("2"));
    expect(screen.getByText("29.5.2")).toBeInTheDocument();
    expect(within(screen.getByLabelText("Disk usage")).getByText("3.0 GB (1.0 GB reclaimable)")).toBeInTheDocument();
  });

  it("updates counts and the event feed live from Docker events", async () => {
    let running = 2;
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/info": () => json(200, { ...info, ContainersRunning: running }),
    });
    const { screen } = renderApp("/envs/local");
    await screen.findByText("2 running · 0 paused · 1 stopped");
    const { sock, sub } = await eventsSub();
    running = 3;
    act(() =>
      sock.receive({
        sub,
        type: "data",
        data: { kind: "container", action: "start", id: "c1", name: "web", attributes: {}, time: 1790000000 },
      }),
    );
    expect(await screen.findByText("container start")).toBeInTheDocument();
    expect(await screen.findByText("3 running · 0 paused · 1 stopped")).toBeInTheDocument();
  });

  it("shows why an environment is down", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/status": { json: { state: "down", reason: "socket not found" } } });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByRole("alert")).toHaveTextContent("socket not found");
  });

  it("follows env_status frames from the server", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/info": { json: info } });
    const { screen } = renderApp("/envs/local");
    await screen.findByText("29.5.2");
    const { sock } = await eventsSub();
    act(() => sock.receive({ type: "env_status", env: "local", status: { state: "down", reason: "daemon stopped" } }));
    expect(await screen.findByRole("alert")).toHaveTextContent("daemon stopped");
  });

  it("shows the error when engine info fails", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/info": { status: 502, json: { code: "upstream", message: "Docker said no" } },
    });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByText("Docker said no", {}, { timeout: 3000 })).toBeInTheDocument();
  });

  it("recovers when the environment comes back up", async () => {
    let up = false;
    const UP = { state: "up", version: { version: "29.5.2", api_version: "1.52", os: "linux", arch: "arm64" } };
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/status": () => json(200, up ? UP : { state: "down", reason: "socket not found" }),
      "GET /api/v1/envs/local/info": () =>
        up ? json(200, info) : json(503, { code: "env_unavailable", message: "docker is down" }),
    });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByRole("alert")).toHaveTextContent("socket not found");
    await waitFor(() => expect(FakeWebSocket.instances.length).toBeGreaterThan(0));
    const sock = FakeWebSocket.latest();
    act(() => sock.open());
    // The server refuses an events subscription while the environment is down.
    for (const f of sock.frames().filter((f) => f.op === "sub" && f.topic === "events")) {
      act(() => sock.receive({ sub: f.sub, type: "error", code: "env_unavailable", message: "docker is down" }));
    }
    const before = sock.frames().length;
    up = true;
    act(() => sock.receive({ type: "env_status", env: "local", status: UP }));
    expect(await screen.findByText("29.5.2")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByLabelText("Containers")).toHaveTextContent("3"));
    await waitFor(() =>
      expect(sock.frames().slice(before).some((f) => f.op === "sub" && f.topic === "events" && f.env === "local")).toBe(true),
    );
  });
});
