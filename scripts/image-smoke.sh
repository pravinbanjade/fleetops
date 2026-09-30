#!/bin/sh
# Smoke-tests a FleetOps image against the local Docker daemon: it starts
# the image with the daemon's socket, then checks that it serves the web
# UI, that it runs as a non-root user, and that the admin it sets up can
# reach Docker through the socket.
#
# Usage: scripts/image-smoke.sh [image]    (default fleetops:dev)
set -eu

image=${1:-fleetops:dev}
name=fleetops-smoke-$$
jar=$(mktemp)
trap 'docker rm -f "$name" >/dev/null 2>&1 || true; rm -f "$jar"' EXIT

fail() {
  echo "image-smoke: $*" >&2
  docker logs "$name" >&2 2>&1 || true
  exit 1
}

# The group that owns the socket inside the daemon's machine. Read it from a
# container, because on Docker Desktop and Colima the host path differs.
gid=$(docker run --rm -v /var/run/docker.sock:/docker.sock busybox:1.37 stat -c %g /docker.sock)

docker run -d --name "$name" -p 127.0.0.1::9000 \
  -v /var/run/docker.sock:/var/run/docker.sock --group-add "$gid" "$image" >/dev/null
base="http://$(docker port "$name" 9000/tcp | head -n1)"

i=0
until curl -fsS "$base/api/v1/health" >/dev/null 2>&1; do
  i=$((i + 1))
  [ "$i" -lt 30 ] || fail "no answer from $base/api/v1/health"
  sleep 1
done

[ "$(docker inspect -f '{{.Config.User}}' "$name")" = nonroot ] || fail "the image does not run as nonroot"
curl -fsS "$base/" | grep -q '/assets/' || fail "/ does not serve the built web UI"

curl -fsS -c "$jar" -H 'content-type: application/json' -H "Origin: $base" \
  -d '{"username":"admin","password":"smoke-password-1"}' "$base/api/v1/auth/setup" >/dev/null ||
  fail "first-run setup failed"
curl -fsS -b "$jar" "$base/api/v1/envs/local/info" | grep -q '"ServerVersion"' ||
  fail "the local environment cannot reach Docker through the socket"

echo "image-smoke: $image ok ($base)"
