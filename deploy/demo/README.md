# The public demo

The read-only FleetOps demo at `https://demo.fleetops.run`. Visitors sign in with a shared account, shown on the login page. That account can view everything on one Docker host, but it cannot make changes, open shells, change its password or see other visitors' sessions. Everything is reset every hour.

## Why FleetOps runs on the host, not in a container

A Viewer can browse and download files from containers and reveal their environment variables. If FleetOps ran as a container on the daemon it manages, visitors could download its own database from `/data`. So the FleetOps binary and Caddy run under systemd on the host. The Docker daemon holds only the sample apps in `shop.yml`, and none of their values are real.

Keep it that way: never run anything else on the demo's Docker daemon.

## Server

A small Linux VM (1 vCPU, 1–2 GB) with Docker Engine and the compose plugin, `jq`, `curl` and Caddy. Point `demo.fleetops.run` at it and open ports 80 and 443 only.

## Install

As root, with a FleetOps release that has demo mode (`FLEETOPS_DEMO_LOGIN`):

```sh
VERSION=v1.5.0   # the first release with demo mode
ARCH=amd64       # or arm64
curl -fsSL -o /usr/local/bin/fleetops \
  "https://github.com/pravinbanjade/fleetops/releases/download/$VERSION/fleetops-$VERSION-linux-$ARCH"
chmod +x /usr/local/bin/fleetops

mkdir -p /opt/fleetops-demo /etc/fleetops-demo
cp shop.yml reset.sh /opt/fleetops-demo/
chmod 700 /opt/fleetops-demo/reset.sh
printf 'FLEETOPS_DEMO_LOGIN=demo:fleetops-demo\n' >/etc/fleetops-demo/env
chmod 600 /etc/fleetops-demo/env

cp fleetops-demo.service fleetops-demo-reset.service fleetops-demo-reset.timer /etc/systemd/system/
cp Caddyfile /etc/caddy/Caddyfile
systemctl daemon-reload
systemctl enable fleetops-demo fleetops-demo-reset.timer
systemctl reload caddy

/opt/fleetops-demo/reset.sh       # the first reset seeds everything
systemctl start fleetops-demo-reset.timer
```

The demo login is the value in `/etc/fleetops-demo/env`. The server shows it on the login page, and the marketing site shows it next to the demo link. If you change it, change it in both places.

## Operating it

- **Admin access:** each reset writes a new random admin password to `/etc/fleetops-demo/admin-password`. Port 9000 is bound to localhost, so reach the admin view through an SSH tunnel: `ssh -L 9000:127.0.0.1:9000 demo-host`.
- **Upgrading:** replace `/usr/local/bin/fleetops` and run `reset.sh`.
- **Logs:** `journalctl -u fleetops-demo` and `journalctl -u fleetops-demo-reset`.
- **Rate limits:** behind Caddy, every visitor reaches FleetOps from 127.0.0.1. The correct demo login is never rate limited, so one visitor's typos can't lock everyone else out. The admin login is still limited, so a burst of wrong admin passwords can lock the admin out for five minutes. Use the SSH tunnel.
