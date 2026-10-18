#!/bin/sh
# Starts (or reuses) the MinIO container the S3 backup tests run against, on
# 127.0.0.1:9100, with the bucket fleetops-it.
set -eu
name=fleetops-minio-it
image=minio/minio:RELEASE.2025-09-07T16-13-09Z
command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }
if ! docker ps --format '{{.Names}}' | grep -qx "$name"; then
  docker rm -f "$name" >/dev/null 2>&1 || true
  docker pull -q "$image" >/dev/null
  docker run -d --name "$name" -p 127.0.0.1:9100:9000 \
    -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
    "$image" server /data >/dev/null
fi
i=0
until curl -fsS http://127.0.0.1:9100/minio/health/ready >/dev/null 2>&1; do
  i=$((i + 1))
  [ "$i" -lt 60 ] || { echo "MinIO did not start" >&2; docker logs "$name" >&2; exit 1; }
  sleep 1
done
docker exec "$name" sh -c 'mc alias set it http://127.0.0.1:9000 minioadmin minioadmin >/dev/null && mc mb --ignore-existing it/fleetops-it >/dev/null'
echo "MinIO ready on http://127.0.0.1:9100 (bucket fleetops-it)"
