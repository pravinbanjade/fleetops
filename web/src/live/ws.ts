import type { ClientMsg, EnvStatus, ServerMsg, Topic } from "@/api/gen/types.gen";

type EndMsg = Extract<ServerMsg, { type: "end" }>;
type ErrorMsg = Extract<ServerMsg, { type: "error" }>;

export interface SubSpec {
  topic: Topic;
  env?: string;
  id?: string;
  opts?: unknown;
}

export interface SubHandlers {
  onData(data: unknown): void;
  /** Exec output: the payload of a binary frame for this subscription. */
  onBinary?(bytes: Uint8Array): void;
  onGap?(dropped: number): void;
  /** The server ended the subscription; it is no longer active. */
  onEnd?(msg: EndMsg): void;
  /** The server refused or failed the subscription; it is no longer active. */
  onError?(msg: ErrorMsg): void;
  /**
   * The connection was re-established and the subscription is about to be
   * sent again. Returning a spec sends that instead, from then on: a log
   * can ask to start after the last line it holds.
   */
  onResubscribe?(): SubSpec | void;
  /**
   * The connection dropped and, because it was made with
   * `resubscribe: false`, the subscription is over.
   */
  onLost?(): void;
}

export interface SubOptions {
  /**
   * Send the subscription again after a reconnect (the default). An exec
   * session cannot resume, so a terminal passes false: resending it would
   * silently start a new process.
   */
  resubscribe?: boolean;
}

/**
 * `connecting` until the first socket opens; `reconnecting` from any
 * unexpected close until a socket opens again.
 */
export type ConnState = "connecting" | "open" | "reconnecting";

export interface WsOptions {
  url: string;
  WebSocketImpl?: typeof WebSocket;
  /** Delay before reconnect attempt `n` (0-based). */
  backoff?: (attempt: number) => number;
  onState?(state: ConnState): void;
  onEnvStatus?(env: string, status: EnvStatus): void;
  /** Called after a reconnect, once every subscription has been sent again. */
  onReconnect?(): void;
  /**
   * The server ended the connection because the session is gone; the
   * client stops reconnecting.
   */
  onUnauthorized?(): void;
  /**
   * A socket closed without ever opening. The server refuses the upgrade
   * with a 401 when the session is gone, which a browser cannot see, so the
   * caller can check the session over HTTP.
   */
  onConnectFailed?(): void;
}

export const defaultBackoff = (attempt: number) => Math.min(30_000, 500 * 2 ** attempt);

export function wsUrl(loc: Location = window.location) {
  return `${loc.protocol === "https:" ? "wss" : "ws"}://${loc.host}/api/v1/ws`;
}

interface Active {
  spec: SubSpec;
  handlers: SubHandlers;
  resubscribe: boolean;
}

/**
 * The tab's single websocket to `/api/v1/ws`. It multiplexes numbered
 * subscriptions, queues them until the socket is open, and on an
 * unexpected close reconnects with backoff and sends every active
 * subscription again.
 */
export class WsClient {
  private readonly opts: WsOptions;
  private socket: WebSocket | null = null;
  private readonly subs = new Map<number, Active>();
  private nextSub = 1;
  private attempt = 0;
  private opened = false;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private stopped = false;

  constructor(opts: WsOptions) {
    this.opts = opts;
  }

  /** Opens the connection; `close()` ends it and a later `start()` reopens it. */
  start() {
    if (this.socket || this.timer) return;
    this.stopped = false;
    this.connect();
  }

  subscribe(
    spec: SubSpec,
    handlers: SubHandlers,
    { resubscribe = true }: SubOptions = {},
  ): { sub: number; unsubscribe: () => void } {
    const sub = this.nextSub++;
    this.subs.set(sub, { spec, handlers, resubscribe });
    this.sendSub(sub, spec);
    return {
      sub,
      unsubscribe: () => {
        if (this.subs.delete(sub)) this.send({ op: "unsub", sub });
      },
    };
  }

  /** Sends a text frame if the socket is open; returns whether it was sent. */
  send(msg: ClientMsg): boolean {
    if (this.socket?.readyState !== WebSocket.OPEN) return false;
    this.socket.send(JSON.stringify(msg));
    return true;
  }

  /** Sends exec stdin as a binary frame: `[u32 sub, big-endian][bytes]`. */
  sendBinary(sub: number, bytes: Uint8Array): boolean {
    if (this.socket?.readyState !== WebSocket.OPEN) return false;
    const frame = new Uint8Array(4 + bytes.length);
    new DataView(frame.buffer).setUint32(0, sub);
    frame.set(bytes, 4);
    this.socket.send(frame);
    return true;
  }

  close() {
    this.stopped = true;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    const socket = this.socket;
    this.socket = null;
    this.opened = false;
    socket?.close();
  }

  private sendSub(sub: number, spec: SubSpec) {
    this.send({ op: "sub", sub, topic: spec.topic, env: spec.env, id: spec.id, opts: spec.opts });
  }

  private connect() {
    const Impl = this.opts.WebSocketImpl ?? WebSocket;
    const socket = new Impl(this.opts.url);
    socket.binaryType = "arraybuffer";
    this.socket = socket;
    if (!this.opened && this.attempt === 0) this.opts.onState?.("connecting");
    let socketOpened = false;
    socket.onopen = () => {
      socketOpened = true;
      const reconnect = this.opened;
      this.opened = true;
      this.attempt = 0;
      this.opts.onState?.("open");
      for (const [sub, active] of this.subs) {
        if (reconnect) active.spec = active.handlers.onResubscribe?.() ?? active.spec;
        this.sendSub(sub, active.spec);
      }
      if (reconnect) this.opts.onReconnect?.();
    };
    socket.onmessage = (ev: MessageEvent) => this.receive(ev.data);
    socket.onclose = () => {
      if (this.socket !== socket) return;
      this.socket = null;
      if (this.stopped) return;
      // Only a subscription that reached an open socket is lost; one still
      // queued is sent once a socket opens.
      if (socketOpened) {
        for (const [sub, a] of this.subs) {
          if (a.resubscribe) continue;
          this.subs.delete(sub);
          a.handlers.onLost?.();
        }
      }
      if (!socketOpened) this.opts.onConnectFailed?.();
      if (this.attempt === 0) this.opts.onState?.("reconnecting");
      const delay = (this.opts.backoff ?? defaultBackoff)(this.attempt++);
      this.timer = setTimeout(() => {
        this.timer = null;
        this.connect();
      }, delay);
    };
  }

  private receive(data: unknown) {
    if (data instanceof ArrayBuffer) {
      if (data.byteLength < 4) return;
      const sub = new DataView(data).getUint32(0);
      this.subs.get(sub)?.handlers.onBinary?.(new Uint8Array(data, 4));
      return;
    }
    let msg: ServerMsg;
    try {
      msg = JSON.parse(String(data)) as ServerMsg;
    } catch {
      return;
    }
    if (msg.type === "env_status") {
      this.opts.onEnvStatus?.(msg.env, msg.status);
      return;
    }
    const sub = msg.sub;
    if (msg.type === "error" && sub == null && msg.code === "unauthorized") {
      this.close();
      this.opts.onUnauthorized?.();
      return;
    }
    if (sub == null) return;
    const active = this.subs.get(sub);
    if (!active) return;
    switch (msg.type) {
      case "data":
        active.handlers.onData(msg.data);
        break;
      case "gap":
        active.handlers.onGap?.(msg.dropped);
        break;
      case "end":
        this.subs.delete(sub);
        active.handlers.onEnd?.(msg);
        break;
      case "error":
        this.subs.delete(sub);
        active.handlers.onError?.(msg);
        break;
    }
  }
}
