# Notifications

Enterprise. FleetOps tells your team about new vulnerabilities in the images you run, on Slack, by email, on Microsoft Teams or Discord, through PagerDuty, or to a webhook of your own. It builds on [image scanning](security-scanning.md): turn scanning on first.

Nothing is sent until an admin adds a **destination** and a **rule** in **Admin → Notifications**.

## Destinations

Each destination is somewhere messages go. Its URL, key or secret is stored encrypted and never shown again; the page shows only the host.

| Kind | What to paste | Where to get it |
|---|---|---|
| Slack | Incoming-webhook URL | Slack app settings → *Incoming Webhooks* → *Add New Webhook to Workspace* |
| Microsoft Teams | Webhook URL | In the channel: *Workflows* → *Post to a channel when a webhook request is received* |
| Discord | Webhook URL | Channel settings → *Integrations* → *Webhooks* → *New Webhook* |
| PagerDuty | Routing key | Service → *Integrations* → *Events API v2* → *Integration Key* |
| Email | Recipients | Uses the SMTP settings below |
| Generic webhook | URL, optional signing secret | Your own tooling |

**Send test** sends a sample marked `[Test]` and shows the error if it fails.

### Email (SMTP)

Enter your mail server or relay: host, port, security (STARTTLS, TLS or none), username, password and the From address. **Amazon SES** works through its SMTP endpoint: `email-smtp.<region>.amazonaws.com`, port 587, STARTTLS, with SES SMTP credentials (not your AWS access keys). SendGrid, Postmark and company relays work the same way.

## Rules

A rule picks what to send where:

- **Environments**: some, or every environment when none is picked.
- **Minimum severity**: critical, high, medium or low.
- **Only when a fix is available** and **only images with running containers** (both on by default).
- **Send to**: one or more destinations.

For example, *Prod → on-call*: production environments, critical, fix only, running only, to the on-call Slack channel; *Dev → security*: dev environments, high and above, to the security channel.

## What gets sent

- **New CVE**: a CVE newly found on an image the rule already knows. One message per CVE, listing every affected image in the rule's environments with where it runs, the package and the fixed version.
- **New image**: an image (repository) the rule hasn't seen arrives with matching CVEs — deployed for the first time, or promoted from dev to production. One message for the image, listing its CVEs.

FleetOps tells a rule once per repository and CVE: a rebuild or redeploy that still carries a CVE you were told about stays quiet. Messages link back to FleetOps when `FLEETOPS_PUBLIC_URL` is set.

**No flood when you start.** When scanning is turned on for an environment, or a rule is created or widened, what is already there is recorded as known without sending anything. Alerts start with what is new after that.

**Batching.** Findings from a wave of scans are grouped: a message goes out two minutes after the last related finding, or ten minutes after the first, so one CVE across 30 images is one message.

**When.** Images are re-scanned daily, so a CVE published overnight reaches you within a day.

## Delivery and retries

The page lists recent deliveries. A failed send is retried after 2 minutes and again after 12; then it is marked failed and its destination shows **Failing** with the error. Other destinations are not held up.

## Generic webhook format

`POST` with `Content-Type: application/json`:

```json
{"version": 1, "event": "cve.new",
 "rule": {"id": 3, "name": "Prod → on-call"},
 "cve": {"id": "CVE-2024-5535", "severity": "critical", "fixed_version": "3.0.14-1~deb12u2", "title": "…",
         "packages": [{"name": "libssl3", "installed_version": "3.0.11-1~deb12u2"}]},
 "images": [{"image_ref": "nginx:1.25.3", "repository": "nginx", "envs": ["prod"], "running": 12}],
 "more": 0}
```

`event` is `image.new` for a new image, with `image` (`repository`, `image_ref`, `envs`, `running`) and `cves` instead.

With a signing secret, `X-FleetOps-Signature: sha256=<hex>` is the HMAC-SHA256 of the raw body. To check it:

```python
import hashlib, hmac
expected = "sha256=" + hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
ok = hmac.compare_digest(expected, request.headers["X-FleetOps-Signature"])
```

PagerDuty gets an Events API v2 `trigger` with `dedup_key` `fleetops:<rule>:<CVE>` (or `fleetops:<rule>:image:<repository>`), so repeats update one incident; severity maps critical→critical, high→error, medium→warning, low→info.
