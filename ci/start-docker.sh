#!/bin/sh
# Starts a Docker daemon inside the CI job's own container (ci/Dockerfile)
# and waits for it. The log is at /tmp/dockerd.log.
set -eu

dind dockerd --host=unix:///var/run/docker.sock >/tmp/dockerd.log 2>&1 &
i=0
until docker info >/dev/null 2>&1; do
  i=$((i + 1))
  if [ "$i" -ge 60 ]; then
    echo "start-docker: the daemon did not start" >&2
    cat /tmp/dockerd.log >&2
    exit 1
  fi
  sleep 1
done
docker version --format 'start-docker: Docker {{.Server.Version}}'
