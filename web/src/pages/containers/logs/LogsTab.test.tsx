import { act, fireEvent, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { loggedIn, renderApp } from "@/test/app";
import { inspect } from "@/test/docker";

const WEB = inspect({ Id: "c1abcdef0123456789", Name: "/web" });
const STOPPED = inspect({
  Id: "c1abcdef0123456789",
  Name: "/web",
  State: { Status: "exited", Running: false, ExitCode: 0 },
});

const at = (sec: number, text: string, stream = "stdout") => ({
  stream,
  text: `2026-09-30T10:00:${String(sec).padStart(2, "0")}.000000001Z ${text}`,
});

/** Opens the page's socket and returns it with the newest logs subscription. */
async function logsSub(sock = FakeWebSocket.latest(), after = 0) {
  if (sock.readyState !== FakeWebSocket.OPEN) act(() => sock.open());
  let frame: { sub: number; opts: Record<string, unknown> } | undefined;
  await waitFor(() => {
    frame = sock.frames().filter((f) => f.op === "sub" && f.topic === "logs" && f.sub > after).pop();
    expect(frame).toBeTruthy();
  });
  return { sock, sub: frame!.sub, opts: frame!.opts };
}

async function openLogs(inspectBody = WEB) {
  mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers/c1": { json: inspectBody } });
  const { screen } = renderApp("/envs/local/containers/c1?tab=logs");
  const log = await screen.findByRole("log", { name: "Container logs" });
  return { screen, log, ...(await logsSub()) };
}

describe("logs tab", () => {
  it("follows the log with timestamps requested, and renders colours and stderr", async () => {
    const { log, sock, sub, opts } = await openLogs();
    expect(opts).toEqual({ follow: true, timestamps: true, tail: 1000, since: null, stdout: true, stderr: true });
    act(() =>
      sock.receive({ sub, type: "data", data: [at(1, "\x1b[31mred alert\x1b[0m"), at(2, "disk full", "stderr")] }),
    );
    const red = await within(log).findByText("red alert");
    expect(red).toHaveStyle({ color: "var(--ansi-1)" });
    expect(within(log).getByText("disk full").parentElement).toHaveClass("border-destructive/70");
    expect(within(log).queryByText(/2026-09-30/)).toBeNull();
  });

  it("shows timestamps when asked, without asking the server again", async () => {
    const { screen, log, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "data", data: [at(1, "hello")] }));
    await within(log).findByText("hello");
    await userEvent.click(screen.getByRole("checkbox", { name: "Timestamps" }));
    expect(within(log).getByText("2026-09-30 10:00:01.000Z")).toBeInTheDocument();
    expect(sock.frames().filter((f) => f.topic === "logs")).toHaveLength(1);
  });

  it("starts over when the tail or the streams change", async () => {
    const { screen, log, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "data", data: [at(1, "old line")] }));
    await within(log).findByText("old line");
    await userEvent.selectOptions(screen.getByLabelText("Lines"), "All lines");
    const next = await logsSub(sock, sub);
    expect(next.opts).toMatchObject({ tail: null });
    expect(sock.frames()).toContainEqual({ op: "unsub", sub });
    expect(within(log).queryByText("old line")).toBeNull();
    await userEvent.click(screen.getByRole("checkbox", { name: "stderr" }));
    expect((await logsSub(sock, next.sub)).opts).toMatchObject({ tail: null, stderr: false, stdout: true });
  });

  it("asks for lines since the chosen time", async () => {
    const { screen, sock, sub } = await openLogs();
    const now = Math.floor(Date.now() / 1000);
    await userEvent.selectOptions(screen.getByLabelText("Since"), "Last hour");
    const since = (await logsSub(sock, sub)).opts.since as number;
    expect(since).toBeGreaterThanOrEqual(now - 3600);
    expect(since).toBeLessThanOrEqual(now - 3600 + 5);
  });

  it("resumes after a reconnect without repeating lines", async () => {
    const { log, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "data", data: [at(1, "one"), at(2, "two")] }));
    await within(log).findByText("two");
    act(() => sock.drop());
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(2), { timeout: 2000 });
    const again = await logsSub(FakeWebSocket.latest());
    // The same subscription is sent again and the server replays its tail.
    act(() =>
      again.sock.receive({ sub: again.sub, type: "data", data: [at(1, "one"), at(2, "two"), at(3, "three")] }),
    );
    await within(log).findByText("three");
    expect(within(log).getAllByText("one")).toHaveLength(1);
    expect(within(log).getAllByText("two")).toHaveLength(1);
  });

  it("says when the container is stopped, and picks up when it starts again", async () => {
    let body = STOPPED;
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers/c1": () => json(200, body) });
    const { screen } = renderApp("/envs/local/containers/c1?tab=logs");
    const log = await screen.findByRole("log", { name: "Container logs" });
    const { sock, sub } = await logsSub();
    act(() => {
      sock.receive({ sub, type: "data", data: [at(1, "last words")] });
      sock.receive({ sub, type: "end", reason: "eof" });
    });
    expect(await screen.findByText(/The container is not running/)).toBeInTheDocument();
    body = WEB;
    const events = sock.frames().find((f) => f.topic === "events").sub;
    act(() =>
      sock.receive({ sub: events, type: "data", data: { kind: "container", action: "start", id: "c1", attributes: {}, time: 2 } }),
    );
    const next = await logsSub(sock, sub);
    await waitFor(() => expect(screen.queryByText(/The container is not running/)).toBeNull());
    act(() => sock.receive({ sub: next.sub, type: "data", data: [at(1, "last words"), at(5, "back up")] }));
    await within(log).findByText("back up");
    expect(within(log).getAllByText("last words")).toHaveLength(1);
  });

  it("keeps a 250k-line log to the newest 200k and renders only the rows in view", async () => {
    const { log, sock, sub } = await openLogs();
    const lines = Array.from({ length: 250_000 }, (_, i) => ({ stream: "stdout", text: `line ${i}` }));
    act(() => sock.receive({ sub, type: "data", data: lines }));
    // jsdom does not scroll, so the view shows the oldest line kept.
    expect(await within(log).findByText("line 50000")).toBeInTheDocument();
    expect(within(log).queryByText("line 49999")).toBeNull();
    expect(log.querySelectorAll("[data-index]").length).toBeLessThan(100);
    expect((log.firstElementChild as HTMLElement).style.height).toBe(`${200_000 * 20}px`);
  });

  it("lays rows out at the fixed height again after wrapping is turned off", async () => {
    const { screen, log, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "data", data: [at(1, "a"), at(2, "b"), at(3, "c")] }));
    await within(log).findByText("c");
    // Wrapped rows are measured; jsdom measures them as 0px tall.
    await userEvent.click(screen.getByRole("checkbox", { name: "Wrap lines" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "Wrap lines" }));
    const tops = [...log.querySelectorAll<HTMLElement>("[data-index]")].map((r) => r.style.transform);
    expect(tops).toEqual(["translateY(0px)", "translateY(20px)", "translateY(40px)"]);
  });

  it("follows a restart it never saw as stopped", async () => {
    let body = WEB;
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers/c1": () => json(200, body) });
    const { screen } = renderApp("/envs/local/containers/c1?tab=logs");
    const log = await screen.findByRole("log", { name: "Container logs" });
    const { sock, sub } = await logsSub();
    act(() => {
      sock.receive({ sub, type: "data", data: [at(1, "first run")] });
      sock.receive({ sub, type: "end", reason: "eof" });
    });
    // The refetch after `die` and `start` lands once it is running again.
    body = inspect({
      Id: "c1abcdef0123456789",
      Name: "/web",
      State: { Status: "running", Running: true, StartedAt: "2026-09-30T10:05:00Z", ExitCode: 0 },
    });
    const events = sock.frames().find((f) => f.topic === "events").sub;
    act(() =>
      sock.receive({ sub: events, type: "data", data: { kind: "container", action: "start", id: "c1", attributes: {}, time: 2 } }),
    );
    const next = await logsSub(sock, sub);
    act(() => sock.receive({ sub: next.sub, type: "data", data: [at(1, "first run"), at(9, "second run")] }));
    await within(log).findByText("second run");
    expect(within(log).getAllByText("first run")).toHaveLength(1);
    expect(screen.queryByText("The log stream ended.")).toBeNull();
  });

  it("resumes from the last line held, not from the tail, so nothing logged meanwhile is missed", async () => {
    const { log, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "data", data: [at(1, "one"), at(2, "two")] }));
    await within(log).findByText("two");
    act(() => sock.drop());
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(2), { timeout: 2000 });
    const again = await logsSub(FakeWebSocket.latest());
    expect(again.opts).toEqual({
      follow: true,
      timestamps: true,
      tail: null,
      since: Math.floor(Date.parse("2026-09-30T10:00:02Z") / 1000),
      stdout: true,
      stderr: true,
    });
  });

  it("marks lines the server skipped", async () => {
    const { log, sock, sub } = await openLogs();
    act(() => {
      sock.receive({ sub, type: "data", data: [at(1, "before")] });
      sock.receive({ sub, type: "gap", dropped: 250 });
    });
    expect(await within(log).findByText(/250 lines skipped/)).toBeInTheDocument();
  });

  it("pauses following when scrolled up and resumes at the bottom", async () => {
    const { screen, log } = await openLogs();
    const follow = screen.getByRole("checkbox", { name: "Follow" });
    expect(follow).toBeChecked();
    Object.defineProperty(log, "scrollHeight", { configurable: true, value: 10_000 });
    Object.defineProperty(log, "clientHeight", { configurable: true, value: 600 });
    const scrollTo = (top: number) => {
      log.scrollTop = top;
      fireEvent.scroll(log);
    };
    scrollTo(9_400);
    scrollTo(4_000);
    expect(follow).not.toBeChecked();
    scrollTo(9_400);
    expect(follow).toBeChecked();
  });

  it("clears the lines shown and keeps them cleared after a reconnect", async () => {
    const { screen, log, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "data", data: [at(1, "noise")] }));
    await within(log).findByText("noise");
    await userEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(within(log).queryByText("noise")).toBeNull();
    act(() => sock.drop());
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(2), { timeout: 2000 });
    const again = await logsSub(FakeWebSocket.latest());
    act(() => again.sock.receive({ sub: again.sub, type: "data", data: [at(1, "noise"), at(2, "fresh")] }));
    await within(log).findByText("fresh");
    expect(within(log).queryByText("noise")).toBeNull();
  });

  it("shows the server's error", async () => {
    const { screen, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "error", code: "not_found", message: "No such container: c1" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("No such container: c1");
  });
});

describe("logs search", () => {
  it("highlights matches, counts them and moves between them, pausing follow", async () => {
    const { screen, log, sock, sub } = await openLogs();
    act(() =>
      sock.receive({
        sub,
        type: "data",
        data: [at(1, "GET /health 200"), at(2, "\x1b[31mERROR\x1b[0m db down"), at(3, "ok"), at(4, "error: retry")],
      }),
    );
    await within(log).findByText("ok");
    await userEvent.type(screen.getByLabelText("Search logs"), "error");
    expect(screen.getByText("2 matches")).toBeInTheDocument();
    const marks = log.querySelectorAll("mark");
    expect([...marks].map((m) => m.textContent)).toEqual(["ERROR", "error"]);
    // The match keeps its colour.
    expect(marks[0]).toHaveStyle({ color: "var(--ansi-1)" });
    await userEvent.keyboard("{Enter}");
    expect(screen.getByText("1 of 2")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Follow" })).not.toBeChecked();
    await userEvent.click(screen.getByRole("button", { name: "Next match" }));
    expect(screen.getByText("2 of 2")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Next match" }));
    expect(screen.getByText("1 of 2")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Previous match" }));
    expect(screen.getByText("2 of 2")).toBeInTheDocument();
  });

  it("counts matches in lines that arrive after the search", async () => {
    const { screen, log, sock, sub } = await openLogs();
    await userEvent.type(screen.getByLabelText("Search logs"), "timeout");
    expect(screen.getByText("No matches")).toBeInTheDocument();
    act(() => sock.receive({ sub, type: "data", data: [at(1, "request timeout")] }));
    await within(log).findByText("timeout");
    expect(screen.getByText("1 match")).toBeInTheDocument();
  });

  it("searches with a regular expression and says when it is invalid", async () => {
    const { screen, log, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "data", data: [at(1, "took 120ms"), at(2, "took 9ms")] }));
    await within(log).findByText("took 9ms");
    await userEvent.click(screen.getByRole("checkbox", { name: "Regex" }));
    const box = screen.getByLabelText("Search logs");
    await userEvent.type(box, "\\d{{3}ms");
    expect(screen.getByText("1 match")).toBeInTheDocument();
    await userEvent.clear(box);
    await userEvent.type(box, "(");
    expect(screen.getByText("Invalid regular expression")).toBeInTheDocument();
  });
});

describe("logs download", () => {
  function captureDownloads() {
    const saved: { name: string; blob: Blob }[] = [];
    let last: Blob | null = null;
    // jsdom has no object URLs; each test file runs in its own environment.
    URL.createObjectURL = (b: Blob) => {
      last = b;
      return "blob:log";
    };
    URL.revokeObjectURL = () => {};
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      saved.push({ name: this.download, blob: last! });
    });
    return saved;
  }

  it("saves the lines shown, as plain text", async () => {
    const saved = captureDownloads();
    const { screen, log, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "data", data: [at(1, "\x1b[32mready\x1b[0m")] }));
    await within(log).findByText("ready");
    await userEvent.click(screen.getByRole("button", { name: "Download shown" }));
    expect(saved).toHaveLength(1);
    expect(saved[0].name).toBe("web.log");
    expect(await saved[0].blob.text()).toBe("ready\n");
  });

  it("fetches the whole log for a full download", async () => {
    const saved = captureDownloads();
    const { screen, sock, sub } = await openLogs();
    await userEvent.click(screen.getByRole("checkbox", { name: "Timestamps" }));
    await userEvent.click(screen.getByRole("button", { name: "Download full log" }));
    const full = await logsSub(sock, sub);
    expect(full.opts).toEqual({ follow: false, tail: null, since: null, timestamps: true, stdout: true, stderr: true });
    expect(screen.getByRole("button", { name: /Preparing/ })).toBeDisabled();
    act(() => {
      sock.receive({ sub: full.sub, type: "data", data: [at(1, "first ever")] });
      sock.receive({ sub: full.sub, type: "end", reason: "eof" });
    });
    await waitFor(() => expect(saved).toHaveLength(1));
    expect(await saved[0].blob.text()).toBe("2026-09-30T10:00:01.000000001Z first ever\n");
    expect(screen.getByRole("button", { name: "Download full log" })).toBeEnabled();
  });

  it("says when a full download is missing lines the server skipped", async () => {
    const saved = captureDownloads();
    const { screen, sock, sub } = await openLogs();
    await userEvent.click(screen.getByRole("button", { name: "Download full log" }));
    const full = await logsSub(sock, sub);
    act(() => {
      sock.receive({ sub: full.sub, type: "data", data: [at(1, "early")] });
      sock.receive({ sub: full.sub, type: "gap", dropped: 30 });
      sock.receive({ sub: full.sub, type: "end", reason: "eof" });
    });
    await waitFor(() => expect(saved).toHaveLength(1));
    expect(await screen.findByText("Saved the log, but 30 lines were skipped because the browser fell behind.")).toBeInTheDocument();
  });

  it("cancels a full download when the connection drops, instead of starting it over", async () => {
    const saved = captureDownloads();
    const { screen, sock, sub } = await openLogs();
    await userEvent.click(screen.getByRole("button", { name: "Download full log" }));
    const full = await logsSub(sock, sub);
    act(() => sock.receive({ sub: full.sub, type: "data", data: [at(1, "partial")] }));
    act(() => sock.drop());
    expect(await screen.findByText("The connection dropped, so the download was cancelled.")).toBeInTheDocument();
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(2), { timeout: 2000 });
    const next = FakeWebSocket.latest();
    act(() => next.open());
    await waitFor(() => expect(next.frames().some((f) => f.topic === "logs")).toBe(true));
    expect(next.frames().filter((f) => f.topic === "logs" && f.opts.follow === false)).toEqual([]);
    expect(saved).toEqual([]);
    expect(screen.getByRole("button", { name: "Download full log" })).toBeEnabled();
  });

  it("says why a full download failed", async () => {
    captureDownloads();
    const { screen, sock, sub } = await openLogs();
    await userEvent.click(screen.getByRole("button", { name: "Download full log" }));
    const full = await logsSub(sock, sub);
    act(() => sock.receive({ sub: full.sub, type: "error", code: "env_unavailable", message: "Docker is down" }));
    expect(await screen.findByText("Could not download the log: Docker is down")).toBeInTheDocument();
  });
});
