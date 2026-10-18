#!/bin/sh
# Starts (or reuses) the OpenLDAP container the LDAP tests run against, on
# 127.0.0.1:3389, and seeds it once with ee/crates/fleetops-ee/tests/fixtures/ldap-seed.ldif.
set -eu
name=fleetops-ldap-it
seed="$(dirname "$0")/../ee/crates/fleetops-ee/tests/fixtures/ldap-seed.ldif"
command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }
if ! docker ps --format '{{.Names}}' | grep -qx "$name"; then
  docker rm -f "$name" >/dev/null 2>&1 || true
  docker pull -q osixia/openldap:1.5.0 >/dev/null
  docker run -d --name "$name" -p 127.0.0.1:3389:389 \
    -e LDAP_ORGANISATION=FleetOps -e LDAP_DOMAIN=fleetops.test -e LDAP_ADMIN_PASSWORD=admin \
    -e LDAP_TLS=false osixia/openldap:1.5.0 >/dev/null
fi
search() {
  docker exec "$name" ldapsearch -x -H ldap://localhost -D cn=admin,dc=fleetops,dc=test -w admin \
    -b dc=fleetops,dc=test "$@"
}
i=0
until search -s base dn >/dev/null 2>&1; do
  i=$((i + 1))
  [ "$i" -lt 60 ] || { echo "OpenLDAP did not start" >&2; docker logs "$name" >&2; exit 1; }
  sleep 1
done
if ! search '(uid=alice)' dn | grep -q '^dn: uid=alice'; then
  docker exec -i "$name" ldapadd -x -H ldap://localhost -D cn=admin,dc=fleetops,dc=test -w admin < "$seed" >/dev/null
fi
echo "OpenLDAP ready on ldap://127.0.0.1:3389"
