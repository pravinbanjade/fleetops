#!/bin/sh
# Starts (or reuses) the S3-compatible server the backup tests write to
# (RustFS; MinIO no longer publishes images), on 127.0.0.1:9100, with the
# bucket fleetops-it and the credentials minioadmin / minioadmin.
set -eu
name=fleetops-s3-it
image=rustfs/rustfs:1.0.0
command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }
if ! docker ps --format '{{.Names}}' | grep -qx "$name"; then
  docker rm -f "$name" >/dev/null 2>&1 || true
  docker pull -q "$image" >/dev/null
  docker run -d --name "$name" -p 127.0.0.1:9100:9000 \
    -e RUSTFS_ACCESS_KEY=minioadmin -e RUSTFS_SECRET_KEY=minioadmin \
    "$image" >/dev/null
fi
i=0
until curl -s -o /dev/null http://127.0.0.1:9100/; do
  i=$((i + 1))
  [ "$i" -lt 60 ] || { echo "the S3 server did not start" >&2; docker logs "$name" >&2; exit 1; }
  sleep 1
done
# Create the bucket (a 409 means it already exists).
curl -s -o /dev/null -X PUT --user minioadmin:minioadmin --aws-sigv4 "aws:amz:us-east-1:s3" \
  http://127.0.0.1:9100/fleetops-it
echo "S3 ready on http://127.0.0.1:9100 (bucket fleetops-it)"
