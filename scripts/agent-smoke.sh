#!/bin/sh
# Smoke-tests the server and agent images together on the local daemon:
# the server runs without the Docker socket, the agent runs with it, and
# the agent environment must come up and list containers.
#
# Usage: scripts/agent-smoke.sh [server image] [agent image]
set -eu

server_image=${1:-fleetops:dev}
agent_image=${2:-fleetops-agent:dev}
net=fleetops-smoke-net-$$
srv=fleetops-smoke-srv-$$
agt=fleetops-smoke-agent-$$
jar=$(mktemp)
trap 'docker rm -f "$srv" "$agt" >/dev/null 2>&1 || true; docker network rm "$net" >/dev/null 2>&1 || true; rm -f "$jar"' EXIT

fail() {
  echo "agent-smoke: $*" >&2
  docker logs "$srv" >&2 2>&1 || true
  docker logs "$agt" >&2 2>&1 || true
  exit 1
}

docker network create "$net" >/dev/null
docker run -d --name "$srv" --network "$net" -p 127.0.0.1::9000 "$server_image" >/dev/null
base="http://$(docker port "$srv" 9000/tcp | head -n1)"
i=0
until curl -fsS "$base/api/v1/health" >/dev/null 2>&1; do
  i=$((i + 1)); [ "$i" -lt 30 ] || fail "server did not start"; sleep 1
done

csrf=$(curl -fsS -c "$jar" -H 'content-type: application/json' -H "Origin: $base" \
  -d '{"username":"admin","password":"smoke-password-1"}' "$base/api/v1/auth/setup" |
  sed -n 's/.*"csrf_token":"\([^"]*\)".*/\1/p')
[ -n "$csrf" ] || fail "setup"
token=$(curl -fsS -b "$jar" -H 'content-type: application/json' -H "Origin: $base" -H "x-csrf-token: $csrf" \
  -d '{"name":"remote","kind":"docker-agent"}' "$base/api/v1/envs" | sed -n 's/.*"join_token":"\([^"]*\)".*/\1/p')
[ -n "$token" ] || fail "no join token"

gid=$(docker run --rm -v /var/run/docker.sock:/docker.sock busybox:1.37 stat -c %g /docker.sock)

# From another container the agent's request is plain HTTP from a
# non-loopback address, which the server must refuse...
docker run -d --name "$agt" --network "$net" -v /var/run/docker.sock:/var/run/docker.sock --group-add "$gid" \
  "$agent_image" --server "http://$srv:9000" --join-token "$token" >/dev/null
sleep 3
docker logs "$agt" 2>&1 | grep -q "403" || fail "plain HTTP from another host was not refused"
docker rm -f "$agt" >/dev/null

# ...so run it in the server's network namespace, where it is loopback.
docker run -d --name "$agt" --network "container:$srv" -v /var/run/docker.sock:/var/run/docker.sock --group-add "$gid" \
  "$agent_image" --server "http://127.0.0.1:9000" --join-token "$token" >/dev/null
i=0
until curl -fsS -b "$jar" "$base/api/v1/envs/remote/status" | grep -q '"state":"up"'; do
  i=$((i + 1)); [ "$i" -lt 30 ] || fail "agent environment never came up"; sleep 1
done
curl -fsS -b "$jar" "$base/api/v1/envs/remote/containers" | grep -q "$srv" ||
  fail "remote containers list is missing the server container"
echo "agent-smoke: ok ($base)"
