import { act, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { loggedIn, renderApp } from "@/test/app";
import { inspect } from "@/test/docker";

// xterm.js needs a real layout and canvas; the tests drive a stand-in.
const { FakeTerminal } = vi.hoisted(() => {
  class FakeTerminal {
    static all: FakeTerminal[] = [];
    cols = 80;
    rows = 24;
    options: Record<string, unknown>;
    written = "";
    selection = "";
    disposed = false;
    keyHandler: ((e: Partial<KeyboardEvent>) => boolean) | null = null;
    private dataCb: ((s: string) => void) | null = null;
    private resizeCb: ((s: { cols: number; rows: number }) => void) | null = null;
    constructor(opts: Record<string, unknown>) {
      this.options = { ...opts };
      FakeTerminal.all.push(this);
    }
    static latest() {
      return FakeTerminal.all[FakeTerminal.all.length - 1];
    }
    loadAddon() {}
    open() {}
    focus() {}
    write(b: Uint8Array | string) {
      this.written += typeof b === "string" ? b : new TextDecoder().decode(b);
    }
    onData(cb: (s: string) => void) {
      this.dataCb = cb;
      return { dispose: () => (this.dataCb = null) };
    }
    onResize(cb: (s: { cols: number; rows: number }) => void) {
      this.resizeCb = cb;
      return { dispose: () => (this.resizeCb = null) };
    }
    attachCustomKeyEventHandler(h: (e: Partial<KeyboardEvent>) => boolean) {
      this.keyHandler = h;
    }
    hasSelection() {
      return this.selection !== "";
    }
    getSelection() {
      return this.selection;
    }
    dispose() {
      this.disposed = true;
    }
    // Test controls.
    type(s: string) {
      this.dataCb?.(s);
    }
    resizeTo(cols: number, rows: number) {
      this.cols = cols;
      this.rows = rows;
      this.resizeCb?.({ cols, rows });
    }
  }
  return { FakeTerminal };
});
vi.mock("@xterm/xterm", () => ({ Terminal: FakeTerminal }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));

const RUNNING = inspect({ Id: "c1abcdef0123456789", Name: "/web" });
const STOPPED = inspect({ Id: "c1abcdef0123456789", Name: "/web", State: { Status: "exited", Running: false, ExitCode: 0 } });

/** A binary exec frame: `[u32 sub, big-endian][bytes]`. */
function frame(sub: number, text: string) {
  const bytes = new TextEncoder().encode(text);
  const out = new Uint8Array(4 + bytes.length);
  new DataView(out.buffer).setUint32(0, sub);
  out.set(bytes, 4);
  return out.buffer;
}

async function openTerminal(body = RUNNING) {
  FakeTerminal.all = [];
  mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers/c1": { json: body } });
  const { screen } = renderApp("/envs/local/containers/c1?tab=terminal");
  await screen.findByLabelText("Shell");
  const sock = FakeWebSocket.latest();
  act(() => sock.open());
  return { screen, sock };
}

/** The newest exec subscription sent on `sock`. */
async function execSub(sock: FakeWebSocket, after = 0) {
  let f: { sub: number; opts: Record<string, unknown> } | undefined;
  await waitFor(() => {
    f = sock.frames().filter((x) => x.op === "sub" && x.topic === "exec" && x.sub > after).pop();
    expect(f).toBeTruthy();
  });
  return f!;
}

describe("terminal tab", () => {
  it("starts nothing until asked, then connects with the auto shell at the terminal's size", async () => {
    const { screen, sock } = await openTerminal();
    expect(screen.getByRole("status")).toHaveTextContent("Choose a shell and connect");
    expect(sock.frames().some((f) => f.topic === "exec")).toBe(false);
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    const { sub, opts } = await execSub(sock);
    expect(opts).toEqual({ cmd: null, user: null, cols: 80, rows: 24 });
    expect(screen.getByRole("status")).toHaveTextContent("Starting…");
    act(() => sock.receive(frame(sub, "root@web:/# ")));
    expect(FakeTerminal.latest().written).toBe("root@web:/# ");
    expect(screen.getByRole("status")).toHaveTextContent("Connected");
  });

  it("sends keystrokes as binary frames and tells the process when the terminal resizes", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    const { sub } = await execSub(sock);
    act(() => FakeTerminal.latest().type("ls\r"));
    const sent = sock.sent.at(-1) as Uint8Array;
    expect(new DataView(sent.buffer).getUint32(0)).toBe(sub);
    expect(new TextDecoder().decode(sent.slice(4))).toBe("ls\r");
    act(() => FakeTerminal.latest().resizeTo(120, 40));
    expect(sock.frames().at(-1)).toEqual({ op: "resize", sub, cols: 120, rows: 40 });
  });

  it("runs a custom command as the chosen user", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.selectOptions(screen.getByLabelText("Shell"), "Custom command");
    await userEvent.type(screen.getByLabelText("Command"), 'python3 -c "print(1)"');
    await userEvent.type(screen.getByLabelText("User"), "www-data");
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect((await execSub(sock)).opts).toMatchObject({ cmd: ["python3", "-c", "print(1)"], user: "www-data" });
  });

  it("runs sh when chosen", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.selectOptions(screen.getByLabelText("Shell"), "sh");
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect((await execSub(sock)).opts).toMatchObject({ cmd: ["sh"] });
  });

  it("names a command it cannot parse and starts nothing", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.selectOptions(screen.getByLabelText("Shell"), "Custom command");
    await userEvent.type(screen.getByLabelText("Command"), 'echo "oops');
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(screen.getByRole("alert")).toHaveTextContent('Command: unclosed " quote');
    expect(sock.frames().some((f) => f.topic === "exec")).toBe(false);
  });

  it("reports the exit code and connects again in a new terminal", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    const { sub } = await execSub(sock);
    act(() => sock.receive({ sub, type: "end", reason: "exited", exit_code: 130 }));
    expect(await screen.findByText("The process exited with code 130.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Connect again" }));
    await execSub(sock, sub);
    expect(FakeTerminal.all).toHaveLength(2);
    expect(FakeTerminal.all[0].disposed).toBe(true);
  });

  it("ends the session when the connection drops, and does not start another on reconnect", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    await execSub(sock);
    act(() => sock.drop());
    expect(await screen.findByText("The connection dropped, which ended the session.")).toBeInTheDocument();
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(2), { timeout: 2000 });
    const next = FakeWebSocket.latest();
    act(() => next.open());
    await waitFor(() => expect(next.frames().some((f) => f.topic === "events")).toBe(true));
    expect(next.frames().some((f) => f.topic === "exec")).toBe(false);
    expect(screen.getByRole("button", { name: "Connect again" })).toBeEnabled();
  });

  it("disconnects on request", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    const { sub } = await execSub(sock);
    await userEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    expect(sock.frames()).toContainEqual({ op: "unsub", sub });
    expect(FakeTerminal.latest().disposed).toBe(true);
    expect(screen.getByRole("button", { name: "Connect" })).toBeInTheDocument();
  });

  it("shows why the exec failed", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    const { sub } = await execSub(sock);
    act(() => sock.receive({ sub, type: "error", code: "upstream", message: "container is paused" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("container is paused");
  });

  it("cannot connect to a stopped container", async () => {
    const { screen } = await openTerminal(STOPPED);
    expect(screen.getByRole("status")).toHaveTextContent("Start the container to open a terminal.");
    expect(screen.getByRole("button", { name: "Connect" })).toBeDisabled();
  });

  it("copies the selection with Ctrl+Shift+C and leaves Ctrl+C to the process", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const { screen, sock } = await openTerminal();
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    await execSub(sock);
    const term = FakeTerminal.latest();
    term.selection = "hello";
    expect(term.keyHandler!({ type: "keydown", ctrlKey: true, shiftKey: true, code: "KeyC" })).toBe(false);
    expect(writeText).toHaveBeenCalledWith("hello");
    expect(term.keyHandler!({ type: "keydown", ctrlKey: true, shiftKey: false, code: "KeyC" })).toBe(true);
  });
});
