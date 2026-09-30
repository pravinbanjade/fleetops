import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import type { ExecOptions } from "@/api/gen/types.gen";
import { useLive } from "@/live/LiveProvider";

export type SessionEnd =
  | { kind: "exited"; code: number | null }
  | { kind: "lost" }
  | { kind: "error"; message: string };

/** A terminal is dark in both themes; the palette is the dark log palette. */
const THEME = {
  background: "#0c0c0d",
  foreground: "#e5e5e5",
  cursor: "#e5e5e5",
  selectionBackground: "#3b5bdb80",
  black: "#3b3b3b",
  red: "#ff6b6b",
  green: "#69db7c",
  yellow: "#ffd43b",
  blue: "#74a7ff",
  magenta: "#e599f7",
  cyan: "#66d9e8",
  white: "#d0d0d0",
  brightBlack: "#808080",
  brightRed: "#ff8787",
  brightGreen: "#8ce99a",
  brightYellow: "#ffe066",
  brightBlue: "#a5c4ff",
  brightMagenta: "#f3bcff",
  brightCyan: "#99e9f2",
  brightWhite: "#ffffff",
};

interface Props {
  env: string;
  id: string;
  /** Command and user; the size comes from the terminal. */
  opts: Pick<ExecOptions, "cmd" | "user">;
  onStarted(): void;
  onEnd(end: SessionEnd): void;
}

/**
 * One exec session in an xterm.js terminal. Output arrives as binary
 * frames; keystrokes go back as binary frames; the terminal fits its box
 * and tells the process its new size. The session never resumes after a
 * dropped connection: resending the subscription would start a new
 * process, so it reports `lost` instead. Unmounting ends the session.
 */
export function TerminalView({ env, id, opts, onStarted, onEnd }: Props) {
  const { ws } = useLive();
  const box = useRef<HTMLDivElement>(null);
  const handlers = useRef({ onStarted, onEnd });
  useEffect(() => {
    handlers.current = { onStarted, onEnd };
  });
  const optsKey = JSON.stringify(opts);

  useEffect(() => {
    const el = box.current;
    if (!ws || !el) return;
    const term = new Terminal({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      theme: THEME,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);
    fit.fit();
    // Ctrl+Shift+C copies the selection; plain Ctrl+C still interrupts.
    term.attachCustomKeyEventHandler((e) => {
      if (e.type === "keydown" && e.ctrlKey && e.shiftKey && e.code === "KeyC" && term.hasSelection()) {
        void navigator.clipboard?.writeText(term.getSelection());
        return false;
      }
      return true;
    });

    let started = false;
    let over = false;
    const end = (e: SessionEnd) => {
      over = true;
      term.options.cursorBlink = false;
      handlers.current.onEnd(e);
    };
    const { sub, unsubscribe } = ws.subscribe(
      {
        topic: "exec",
        env,
        id,
        opts: { ...(JSON.parse(optsKey) as Props["opts"]), cols: term.cols, rows: term.rows },
      },
      {
        onData: () => {},
        onBinary: (bytes) => {
          if (!started) {
            started = true;
            handlers.current.onStarted();
          }
          term.write(bytes);
        },
        onEnd: (m) => end({ kind: "exited", code: m.exit_code ?? null }),
        onError: (m) => end({ kind: "error", message: m.message }),
        onLost: () => end({ kind: "lost" }),
      },
      { resubscribe: false },
    );
    const encoder = new TextEncoder();
    const input = term.onData((s) => {
      if (!over) ws.sendBinary(sub, encoder.encode(s));
    });
    const resize = term.onResize(({ cols, rows }) => {
      if (!over) ws.send({ op: "resize", sub, cols, rows });
    });
    const observer = new ResizeObserver(() => fit.fit());
    observer.observe(el);
    term.focus();
    return () => {
      observer.disconnect();
      input.dispose();
      resize.dispose();
      unsubscribe();
      term.dispose();
    };
  }, [ws, env, id, optsKey]);

  return <div ref={box} aria-label="Terminal" className="h-[calc(100vh-20rem)] min-h-80 rounded-md bg-[#0c0c0d] p-2" />;
}
