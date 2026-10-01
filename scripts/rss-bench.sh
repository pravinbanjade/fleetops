#!/bin/sh
# Checks the idle memory target: a FleetOps binary managing a host with
# about 50 containers should hold under 50 MiB of RSS once it is idle.
#
# It starts $COUNT busybox containers on the current Docker daemon, runs the
# binary against it with a fresh data directory, sets up the admin, loads
# every list page's API once (as a browser session would on first visit),
# lets it settle, then samples the process's resident set size.
#
# Usage: scripts/rss-bench.sh [binary]    (default target/release/fleetops)
# Environment: COUNT (default 50), LIMIT_MIB (default 50), SETTLE (seconds, default 10)
set -eu

bin=${1:-target/release/fleetops}
count=${COUNT:-50}
limit_mib=${LIMIT_MIB:-50}
settle=${SETTLE:-10}
label=fleetops.rss-bench=$$
data=$(mktemp -d)
jar=$(mktemp)
log=$(mktemp)
pid=

cleanup() {
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

curl -fsS -c "$jar" -H 'content-type: application/json' -H "Origin: $base" \
  -d '{"username":"admin","password":"rss-bench-password-1"}' "$base/api/v1/auth/setup" >/dev/null ||
  fail "first-run setup failed"

for path in info status df containers images volumes networks events; do
  curl -fsS -b "$jar" "$base/api/v1/envs/local/$path" >/dev/null || fail "GET /envs/local/$path failed"
done

seen=$(curl -fsS -b "$jar" "$base/api/v1/envs/local/containers" | grep -o "fleetops-rss-$$-" | wc -l | tr -d ' ')
[ "$seen" -ge "$count" ] || fail "the containers list shows $seen of $count bench containers"

sleep "$settle"

# The highest of three samples, a second apart. ps reports KiB.
max=0
for _ in 1 2 3; do
  kib=$(ps -o rss= -p "$pid" | tr -d ' ')
  [ -n "$kib" ] || fail "the server exited"
  [ "$kib" -gt "$max" ] && max=$kib
  sleep 1
done

mib=$(awk "BEGIN { printf \"%.1f\", $max / 1024 }")
echo "rss-bench: idle RSS ${mib} MiB with $count containers (limit ${limit_mib} MiB)"
[ "$max" -le $((limit_mib * 1024)) ] || fail "idle RSS ${mib} MiB is over the ${limit_mib} MiB target"
