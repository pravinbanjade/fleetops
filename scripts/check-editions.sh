#!/bin/sh
# Fails when Community code reaches into ee/ (editions spec §6): the CE
# server must not depend on fleetops-ee, CE web code imports the Enterprise
# module only through web/src/ee-slots.tsx, and the CE bundle (make web)
# carries no Enterprise marker. Access lookups go through authz so that
# Enterprise team grants are never skipped.
set -eu
cd "$(dirname "$0")/.."
fail=0
if cargo tree -p fleetops-server -e normal --prefix none | grep -q '^fleetops-ee '; then
  echo "fleetops-server depends on fleetops-ee without the enterprise feature" >&2
  fail=1
fi
hits=$(grep -rnE "[\"'](\.\./)+ee/|[\"']@ee[\"']" web/src web/e2e | grep -v '^web/src/ee-slots.tsx:' || true)
if [ -n "$hits" ]; then
  echo "Community web code imports Enterprise code:" >&2
  echo "$hits" >&2
  fail=1
fi
hits=$(grep -rnE 'store\.(env_grant|user_grants)\(' crates/fleetops-api/src | grep -v '^crates/fleetops-api/src/authz.rs:' || true)
if [ -n "$hits" ]; then
  echo "access lookups must go through authz::effective_grant(s), or team grants are skipped:" >&2
  echo "$hits" >&2
  fail=1
fi
if grep -rq __FLEETOPS_EE__ web/dist; then
  echo "web/dist contains the Enterprise marker; build it with 'make web'" >&2
  fail=1
fi
exit $fail
