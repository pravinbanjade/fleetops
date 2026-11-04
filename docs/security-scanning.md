# Image vulnerability scanning

Enterprise. FleetOps scans the images on your Docker hosts, Swarm nodes and Kubernetes clusters with [Trivy](https://trivy.dev), and the **Security** page lists every image across the fleet with the ones to patch first on top: fixable critical vulnerabilities on images with running containers.

## Turning it on

Scanning is **off by default**. Nothing is pulled and nothing runs until an admin turns it on in **Admin → Security scanning**: for every environment, or per environment (each one can *inherit*, be *on* or be *off*; the same choice is in the environment's panel under **Admin → Environments**).

Turning it on for an environment pulls the Trivy image (pinned by digest; the settings page shows which) and downloads Trivy's vulnerability database. If either fails, the environment shows **Scanner unavailable** with the reason, and its images show as waiting for the scanner, never as clean.

## When images are scanned

- When an image arrives: pulled, built, loaded or tagged, from FleetOps or not. On Kubernetes, when a pod first runs a new image (checked every two minutes).
- Every day, because new vulnerabilities are published for images you already have.
- When someone presses **Scan now**: on an image's **Vulnerabilities** tab, on the Security page for selected images, or for a whole environment.

Results are kept per image digest, so the same image on ten hosts is scanned once, and a tag that moves to a new digest is scanned again. A result older than two days is shown as *stale*.

## How scans run

**Docker hosts, agent hosts and Swarm nodes.** FleetOps starts a short-lived `fleetops-scan-…` container on the host that has the image, through the same connection it already uses. The container mounts the engine socket so Trivy reads the image straight from the host's own storage; private images you have pulled need no registry credentials. The socket is root-equivalent access to the host, the same access FleetOps itself has. Each scan is limited to 1 CPU and 1 GiB of memory, and only one runs per host at a time. Trivy's database is cached in the `fleetops-trivy-cache` volume.

Rootless engines keep their socket at a path FleetOps can't know. For those, FleetOps exports the image instead and streams it into the scan container (it is spooled under the FleetOps data directory while it is copied), or you can set the socket's path on the host in the environment's panel.

**Kubernetes.** FleetOps runs a Job per image in the `fleetops-scan` namespace (1 CPU, 1 GiB, a 20-minute deadline). Its pod runs the image itself, pinned to the digest your pods run, so the cluster pulls it exactly as it does for your workloads: through the nodes' own registry access (for example an EKS node role reading ECR) or the image pull secrets your pods use, which FleetOps copies into the namespace for the scan and deletes afterwards. Nothing in the image runs: its entrypoint is replaced by a Trivy binary that init containers copy in, after refreshing the vulnerability and Java databases. The scan pod gets no service account token, and runs on a node of the same OS and architecture as your pods. The databases are downloaded when scanning is turned on and cached on a 10 GiB `fleetops-trivy-cache` claim; a smaller claim from an earlier version is grown, or replaced if its storage class can't grow. The report is read from the Job's log, so it is limited by the kubelet's container log size (10 MiB by default). A mirror Trivy image must be a copy of the official `aquasec/trivy` (FleetOps copies its `/usr/local/bin/trivy`); turning scanning on checks this. The permissions this needs are in [kubernetes.md](kubernetes.md#image-scanning).

## Air-gapped environments

In **Admin → Security scanning → Mirror** (or per environment), set where Trivy and its databases come from:

- **Trivy image**: your registry's copy of `aquasec/trivy`, ideally pinned by digest.
- **Vulnerability database** and **Java database**: OCI repositories holding mirrors of `ghcr.io/aquasecurity/trivy-db` and `trivy-java-db`, for example copied with `oras copy ghcr.io/aquasecurity/trivy-db:2 mirror.example.com/trivy-db:2`. Keep them updated: an old database misses new vulnerabilities.
- **Username** and **password**, when the mirror needs them. The password is stored encrypted and never shown again.

## Who sees what

- The Security page shows only the environments you can access, and on a cluster only the namespaces you can see pods in.
- **Scan now** needs *create* on images (Docker hosts and swarms) or *operate* on workloads in a namespace that uses the image (clusters).
- Only admins change the settings. Turning scanning on or off, mirror changes, Scan now and failed scans are in the audit log.

## Notifications

To be told about new CVEs instead of watching the page, set up [notifications](notifications.md).

## Turning it off

Turning scanning off for an environment stops new scans and removes the environment's images from the Security page. Results for an image no environment uses any more are deleted after 30 days.
