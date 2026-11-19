#!/bin/sh
# Puts the demo back to a clean state: a fresh FleetOps database with one
# admin (random password, kept root-only) and the shared Viewer account, and
# the sample apps recreated. Run hourly by fleetops-demo-reset.timer.
set -eu

DIR=/opt/fleetops-demo
DATA=/var/lib/fleetops-demo
ENVFILE=/etc/fleetops-demo/env
API=http://127.0.0.1:9000/api/v1

# FLEETOPS_DEMO_LOGIN=user:password, the same value the server shows.
. "$ENVFILE"
DEMO_USER=${FLEETOPS_DEMO_LOGIN%%:*}
DEMO_PW=${FLEETOPS_DEMO_LOGIN#*:}

systemctl stop fleetops-demo
rm -rf "$DATA"
mkdir -p "$DATA"
chmod 700 "$DATA"

docker compose -f "$DIR/shop.yml" down --volumes --remove-orphans
docker compose -f "$DIR/shop.yml" up -d --quiet-pull

systemctl start fleetops-demo
i=0
until curl -fsS "$API/health" >/dev/null 2>&1; do
	i=$((i + 1))
	[ "$i" -lt 60 ] || { echo "FleetOps did not start" >&2; exit 1; }
	sleep 1
done

JAR=$(mktemp)
trap 'rm -f "$JAR"' EXIT
ADMIN_PW=$(head -c 24 /dev/urandom | base64 | tr -d '/+=')
umask 077
printf '%s\n' "$ADMIN_PW" >/etc/fleetops-demo/admin-password

csrf=$(curl -fsS -c "$JAR" -H 'content-type: application/json' \
	-d "$(jq -n --arg p "$ADMIN_PW" '{username: "admin", password: $p}')" "$API/auth/setup" |
	jq -r .csrf_token)

call() { # method path [json]
	curl -fsS -b "$JAR" -X "$1" -H "x-csrf-token: $csrf" -H 'content-type: application/json' \
		${3:+-d "$3"} "$API$2"
}

user_id=$(call POST /users "$(jq -n --arg u "$DEMO_USER" --arg p "$DEMO_PW" \
	'{username: $u, password: $p, role: "user"}')" | jq -r .id)
env_id=$(call GET /envs | jq -r '.[] | select(.name == "local") | .id')
[ -n "$env_id" ] || { echo "no local environment" >&2; exit 1; }
call PUT "/users/$user_id/access/$env_id" '{"role":"viewer"}' >/dev/null
echo "demo reset: $DEMO_USER is a viewer on local"
