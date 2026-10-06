#!/bin/sh
# Checks the idle memory target: a FleetOps binary managing a host with
# about 50 containers should hold under 50 MiB of RSS once it is idle.
#
# It starts $COUNT busybox containers on the current Docker daemon, runs the
# binary against it with a fresh data directory, sets up the admin, loads
# every list page's API once (as a browser session would on first visit),
# lets it settle, then samples the process's resident set size.
#
# Then it enrolls $AGENTS fleetops-agent processes against the same daemon
# and checks the server stays under LIMIT_MIB + 1 MiB per agent and one idle
# agent under AGENT_LIMIT_MIB.
#
# Usage: scripts/rss-bench.sh [binary] [agent binary]
#        (defaults target/release/fleetops, target/release/fleetops-agent)
# Environment: COUNT (default 50), LIMIT_MIB (default 50), SETTLE (seconds, default 10),
#              AGENTS (default 10, 0 skips the agent phase), AGENT_LIMIT_MIB (default 15),
#              KUBE_IT (kind fixtures dir; unset skips the kube phase), KUBE_ENVS (default 5)
set -eu

bin=${1:-target/release/fleetops}
agent_bin=${2:-target/release/fleetops-agent}
agents=${AGENTS:-10}
agent_limit_mib=${AGENT_LIMIT_MIB:-15}
agent_pids=
count=${COUNT:-50}
limit_mib=${LIMIT_MIB:-50}
settle=${SETTLE:-10}
label=fleetops.rss-bench=$$
data=$(mktemp -d)
jar=$(mktemp)
log=$(mktemp)
pid=

cleanup() {
  for a in $agent_pids; do kill "$a" 2>/dev/null || true; done
  [ -z "$pid" ] || kill "$pid" 2>/dev/null || true
  ids=$(docker ps -aq --filter "label=$label")
  [ -z "$ids" ] || docker rm -f $ids >/dev/null 2>&1 || true
  rm -rf "$data" "$jar" "$log"
}
trap cleanup EXIT

fail() {
  echo "rss-bench: $*" >&2
  cat "$log" >&2 || true
  exit 1
}

[ -x "$bin" ] || fail "no binary at $bin (run make build)"

docker pull -q busybox:1.37 >/dev/null
i=0
while [ "$i" -lt "$count" ]; do
  docker run -d --label "$label" --name "fleetops-rss-$$-$i" busybox:1.37 sleep 3600 >/dev/null
  i=$((i + 1))
done

# A free port, picked by the kernel.
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
base="http://127.0.0.1:$port"

DOCKER_HOST=${DOCKER_HOST:-$(docker context inspect --format '{{.Endpoints.docker.Host}}')} \
  FLEETOPS_LOG=warn "$bin" --data "$data" --bind "127.0.0.1:$port" >"$log" 2>&1 &
pid=$!

i=0
until curl -fsS "$base/api/v1/health" >/dev/null 2>&1; do
  i=$((i + 1))
  [ "$i" -lt 30 ] || fail "no answer from $base/api/v1/health"
  sleep 1
done

csrf=$(curl -fsS -c "$jar" -H 'content-type: application/json' -H "Origin: $base" \
  -d '{"username":"admin","password":"rss-bench-password-1"}' "$base/api/v1/auth/setup" |
  sed -n 's/.*"csrf_token":"\([^"]*\)".*/\1/p')
[ -n "$csrf" ] || fail "first-run setup failed"

for path in info status df containers images volumes networks events; do
  curl -fsS -b "$jar" "$base/api/v1/envs/local/$path" >/dev/null || fail "GET /envs/local/$path failed"
done

seen=$(curl -fsS -b "$jar" "$base/api/v1/envs/local/containers" | grep -o "fleetops-rss-$$-" | wc -l | tr -d ' ')
[ "$seen" -ge "$count" ] || fail "the containers list shows $seen of $count bench containers"

sleep "$settle"

# The highest of three samples of process $1, a second apart. ps reports KiB.
peak_rss() {
  peak=0
  for _ in 1 2 3; do
    kib=$(ps -o rss= -p "$1" | tr -d ' ')
    [ -n "$kib" ] || fail "process $1 exited"
    [ "$kib" -gt "$peak" ] && peak=$kib
    sleep 1
  done
  echo "$peak"
}
to_mib() { awk "BEGIN { printf \"%.1f\", $1 / 1024 }"; }

max=$(peak_rss "$pid")
mib=$(to_mib "$max")
echo "rss-bench: idle RSS ${mib} MiB with $count containers (limit ${limit_mib} MiB)"
[ "$max" -le $((limit_mib * 1024)) ] || fail "idle RSS ${mib} MiB is over the ${limit_mib} MiB target"

[ "$agents" -gt 0 ] || exit 0
[ -x "$agent_bin" ] || fail "no agent binary at $agent_bin (cargo build --release -p fleetops-agent)"
docker_url=${DOCKER_HOST:-$(docker context inspect --format '{{.Endpoints.docker.Host}}')}
i=0
while [ "$i" -lt "$agents" ]; do
  token=$(curl -fsS -b "$jar" -H 'content-type: application/json' -H "Origin: $base" -H "x-csrf-token: $csrf" \
    -d "{\"name\":\"rss-agent-$i\",\"kind\":\"docker-agent\"}" "$base/api/v1/envs" |
    sed -n 's/.*"join_token":"\([^"]*\)".*/\1/p')
  [ -n "$token" ] || fail "could not create agent environment $i"
  adata="$data/agent-$i"
  mkdir -p "$adata"
  "$agent_bin" --server "$base" --join-token "$token" --data "$adata" --docker "$docker_url" --log warn >>"$log" 2>&1 &
  agent_pids="$agent_pids $!"
  i=$((i + 1))
done

i=0
until [ "$(curl -fsS -b "$jar" "$base/api/v1/envs" | grep -o '"connected":true' | wc -l | tr -d ' ')" -ge "$agents" ]; do
  i=$((i + 1))
  [ "$i" -lt 30 ] || fail "only some of $agents agents connected"
  sleep 1
done
# Each agent environment answers through its tunnel.
i=0
while [ "$i" -lt "$agents" ]; do
  curl -fsS -b "$jar" "$base/api/v1/envs/rss-agent-$i/containers" >/dev/null || fail "rss-agent-$i is not serving"
  i=$((i + 1))
done

sleep "$settle"

server_kib=$(peak_rss "$pid")
first_agent=$(echo "$agent_pids" | awk '{ print $1 }')
agent_kib=$(peak_rss "$first_agent")
server_limit=$((limit_mib + agents))
echo "rss-bench: server $(to_mib "$server_kib") MiB with $agents agents (limit $server_limit MiB); agent $(to_mib "$agent_kib") MiB (limit $agent_limit_mib MiB)"
[ "$server_kib" -le $((server_limit * 1024)) ] || fail "server RSS is over $server_limit MiB with $agents agents"
[ "$agent_kib" -le $((agent_limit_mib * 1024)) ] || fail "agent RSS is over $agent_limit_mib MiB"

# Kubernetes phase, when a kind cluster's fixtures are at $KUBE_IT
# (scripts/kind-up.sh): kubeconfig environments with their lists polled.
kube_envs=${KUBE_ENVS:-5}
if [ -n "${KUBE_IT:-}" ] && [ "$kube_envs" -gt 0 ]; then
  kubeconfig=$(python3 -c 'import json,sys; print(json.dumps(open(sys.argv[1]).read()))' "$KUBE_IT/kubeconfig")
  i=0
  while [ "$i" -lt "$kube_envs" ]; do
    curl -fsS -b "$jar" -H 'content-type: application/json' -H "Origin: $base" -H "x-csrf-token: $csrf" \
      -d "{\"name\":\"rss-kube-$i\",\"kind\":\"kube-config\",\"kubeconfig\":$kubeconfig}" "$base/api/v1/envs" >/dev/null ||
      fail "could not create kube environment $i"
    i=$((i + 1))
  done
  round=0
  while [ "$round" -lt 3 ]; do
    i=0
    while [ "$i" -lt "$kube_envs" ]; do
      for kind in pods deployments services; do
        curl -fsS -b "$jar" "$base/api/v1/envs/rss-kube-$i/k8s/$kind" >/dev/null || fail "rss-kube-$i cannot list $kind"
      done
      i=$((i + 1))
    done
    round=$((round + 1))
  done
  # Three live watches per environment, held open while measuring.
  cookie="fleetops_session=$(awk '$6 == "fleetops_session" {print $7}' "$jar")"
  watch_envs=$(i=0; while [ "$i" -lt "$kube_envs" ]; do printf 'rss-kube-%s ' "$i"; i=$((i + 1)); done)
  # The admin's watches, then the same watches from namespace-limited
  # users: they share the admin's upstream watches and only filter.
  limited=${LIMITED_USERS:-5}
  cookies="$cookie"
  u=0
  while [ "$u" -lt "$limited" ]; do
    uid=$(curl -fsS -b "$jar" -H 'content-type: application/json' -H "Origin: $base" -H "x-csrf-token: $csrf" \
      -d "{\"username\":\"rss-ns-$u\",\"password\":\"rss-bench-password-1\",\"role\":\"user\"}" "$base/api/v1/users" |
      sed -n 's/.*"id":\([0-9]*\).*/\1/p')
    [ -n "$uid" ] || fail "could not create limited user $u"
    for envname in $watch_envs; do
      env_id=$(curl -fsS -b "$jar" "$base/api/v1/envs" | python3 -c 'import json,sys; print(next(e["id"] for e in json.load(sys.stdin) if e["name"] == sys.argv[1]))' "$envname")
      curl -fsS -X PUT -b "$jar" -H 'content-type: application/json' -H "Origin: $base" -H "x-csrf-token: $csrf" \
        -d '{"role":"viewer","namespaces":["fleetops-it"]}' "$base/api/v1/users/$uid/access/$env_id" >/dev/null ||
        fail "could not grant rss-ns-$u on $envname"
    done
    ujar=$(mktemp)
    curl -fsS -c "$ujar" -H 'content-type: application/json' -H "Origin: $base" \
      -d "{\"username\":\"rss-ns-$u\",\"password\":\"rss-bench-password-1\"}" "$base/api/v1/auth/login" >/dev/null ||
      fail "rss-ns-$u could not log in"
    cookies="$cookies|fleetops_session=$(awk '$6 == "fleetops_session" {print $7}' "$ujar")"
    rm -f "$ujar"
    u=$((u + 1))
  done
  watch_pids=""
  watch_outs=""
  IFS='|'
  for c in $cookies; do
    unset IFS
    out=$(mktemp)
    # shellcheck disable=SC2086
    node "$(dirname "$0")/kube-watch-load.mjs" "$base" "$c" 3 $watch_envs >"$out" 2>&1 &
    watch_pids="$watch_pids $!"
    watch_outs="$watch_outs $out"
    IFS='|'
  done
  unset IFS
  for out in $watch_outs; do
    tries=0
    until grep -q ready "$out"; do
      tries=$((tries + 1))
      [ "$tries" -le 60 ] || fail "kube watches never all got their snapshots: $(cat "$out")"
      sleep 0.5
    done
  done
  sleep "$settle"
  kube_kib=$(peak_rss "$pid")
  # shellcheck disable=SC2086
  kill $watch_pids 2>/dev/null || true
  # shellcheck disable=SC2086
  rm -f $watch_outs
  kube_limit=$((limit_mib + agents + 5))
  echo "rss-bench: server $(to_mib "$kube_kib") MiB with $agents agents and $kube_envs kube environments x 3 watches, for the admin and $limited namespace-limited users (limit $kube_limit MiB)"
  [ "$kube_kib" -le $((kube_limit * 1024)) ] || fail "server RSS is over $kube_limit MiB with $kube_envs kube environments"
fi
