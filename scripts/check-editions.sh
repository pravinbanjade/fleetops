#!/bin/sh
# The source boundary between Apache-2.0 code and ee/ (ee/LICENSE): web code
# outside ee/ imports the Enterprise module only through web/src/ee-slots.tsx,
# and access lookups go through authz so that Enterprise team grants are never
# skipped.
set -eu
cd "$(dirname "$0")/.."
fail=0
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
exit $fail
