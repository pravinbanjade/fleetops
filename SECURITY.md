# Security policy

FleetOps holds the keys to the hosts and clusters it manages: Docker socket access is root on the host, and a stored kubeconfig or agent can be cluster admin. Security reports get priority over everything else.

## Reporting a vulnerability

**Please don't report vulnerabilities in a public issue, merge request or discussion.** Use one of these private channels:

- **GitLab:** open a [confidential issue](https://gitlab.com/pravinbanjade/fleetops/-/issues/new?issue[confidential]=true) (tick *This issue is confidential*).
- **GitHub:** use [private vulnerability reporting](https://github.com/pravinbanjade/fleetops/security/advisories/new) on the mirror.

Include what you can of:

- the FleetOps version (`fleetops --version`) or commit, the edition (Community or Enterprise), and how it runs (binary, image, agent);
- the affected component: server, web UI, agent, Kubernetes agent manifest, or Enterprise licensing;
- steps to reproduce, or a proof of concept;
- the impact as you see it: who can do what they shouldn't be able to.

## What to expect

FleetOps is maintained by one person, so these are aims rather than guarantees:

- an acknowledgement within 7 days;
- an assessment, and a fix plan for confirmed issues, within 30 days;
- a fixed release, then a public advisory that credits you unless you'd rather stay anonymous.

Please give a fix a reasonable chance to ship before disclosing publicly. If a report goes unanswered for 14 days, it's fine to follow up on the same channel.

## Supported versions

FleetOps is in early development (0.x). Only the latest release receives security fixes; upgrade to it before reporting if you can.

## Scope

In scope, for example:

- authentication, session, CSRF or rate-limit bypasses;
- authorization flaws: a viewer acting as an operator, a user reaching an environment or namespace they have no role on, a non-admin reaching admin routes;
- agent tunnel issues: reusing or forging join tokens and agent keys, reaching anything other than the tunnel with agent credentials;
- reading stored kubeconfigs or other sealed secrets without the secret key;
- XSS, CSP bypasses, or Origin-check bypasses on the websocket;
- actions that skip the audit log;
- Enterprise license verification bypasses.

Behaviour that is documented as part of the trust model is not a vulnerability on its own:

- access to the Docker socket, or to the socket group granted with `--group-add`, is root on that host;
- a compromised server is root on every agent host and has the access of every stored kubeconfig and agent;
- secret masking in the UI is display-only, and the API returns values unredacted to users who may read them;
- namespace-limited operators can create pods with whatever service accounts and host access the cluster's own policies allow;
- kubeconfig exec plugins, when enabled with `--kube-exec-plugins`, run as the user running FleetOps;
- the committed test license key (`ee/crates/fleetops-ee/testdata/test.key`) is only trusted in builds with the `test-license-key` feature, which releases never enable.

The [Security section of the README](README.md#security) describes these defences and limits in more detail.
