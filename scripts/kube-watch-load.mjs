// Holds `per` kube-watch subscriptions open on each named environment
// until killed, for rss-bench. Prints "ready" once every subscription has
// its snapshot. Usage: node kube-watch-load.mjs <base> <cookie> <per> env...
const [base, cookie, per, ...envs] = process.argv.slice(2);
const kinds = ["pods", "deployments", "services", "configmaps", "secrets"].slice(0, Number(per));
const want = envs.length * kinds.length;
const ws = new WebSocket(base.replace(/^http/, "ws") + "/api/v1/ws", { headers: { cookie } });
let sub = 0;
let snapshots = 0;
ws.onopen = () => {
  for (const env of envs)
    for (const kind of kinds) ws.send(JSON.stringify({ op: "sub", sub: ++sub, topic: "kube-watch", env, id: `${kind}/-` }));
};
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.type === "error" || m.type === "end") {
    console.error("kube-watch-load:", ev.data);
    process.exit(1);
  }
  if (m.type === "data" && m.data.type === "snapshot" && ++snapshots === want) console.log("ready");
};
ws.onerror = (e) => {
  console.error("kube-watch-load:", e.message ?? e);
  process.exit(1);
};
ws.onclose = () => process.exit(1);
