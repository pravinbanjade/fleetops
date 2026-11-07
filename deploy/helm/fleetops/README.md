# FleetOps Helm chart

Runs the FleetOps server in a Kubernetes cluster: one pod with its SQLite database on a persistent volume, a Service with the web UI on port 9000 and HTTPS for agents on 9443, and an optional Ingress.

## Install

```sh
helm repo add fleetops https://pravinbanjade.github.io/fleetops
helm install fleetops fleetops/fleetops --namespace fleetops --create-namespace
```

Or from the OCI registry, without adding a repository:

```sh
helm install fleetops oci://ghcr.io/pravinbanjade/charts/fleetops --namespace fleetops --create-namespace
```

Then open the UI and create the admin account within 10 minutes of the pod starting:

```sh
kubectl -n fleetops port-forward svc/fleetops 9000:9000   # http://localhost:9000
```

Without Helm, apply the manifest rendered from this chart with its defaults: `kubectl create namespace fleetops && kubectl apply -n fleetops -f https://raw.githubusercontent.com/pravinbanjade/fleetops/main/deploy/kubernetes/fleetops.yaml`.

## Managing the cluster FleetOps runs in

Add it like any other cluster: **Admin → Environments → Add environment → Kubernetes cluster (in-cluster agent)**, and run the `kubectl apply` command it shows. The agent goes into the `fleetops` namespace and dials the URL your browser is on, so open the UI over the address the agent should use first:

- **Same cluster, default install.** Install the release as `fleetops` in the `fleetops` namespace, so the Service is `fleetops`. Port-forward HTTPS (`kubectl -n fleetops port-forward svc/fleetops 9443:9443`), but open the UI as `https://fleetops:9443`, with `127.0.0.1 fleetops` in your `/etc/hosts`. The agent then dials the Service, and the command carries the certificate FleetOps generated for `localhost` and `fleetops`.
- **Through an ingress.** Set `ingress.backendPort=https` (the default) and tell the controller to speak HTTPS to the backend, for ingress-nginx `nginx.ingress.kubernetes.io/backend-protocol: HTTPS`. Agents in any cluster can then use the ingress host.

Agents must reach FleetOps over HTTPS that FleetOps terminates itself: a connection that arrives on the plain-HTTP port is refused unless it comes from loopback. An ingress that ends TLS and forwards plain HTTP (`ingress.backendPort=http`) serves the UI but not agents.

## Examples

Behind ingress-nginx with cert-manager:

```yaml
ingress:
  enabled: true
  className: nginx
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt
    nginx.ingress.kubernetes.io/backend-protocol: HTTPS
  hosts:
    - host: fleetops.example.com
      paths: [{path: /, pathType: Prefix}]
  tls:
    - secretName: fleetops-example-tls
      hosts: [fleetops.example.com]
```

`publicUrl` defaults to the first TLS host, `https://fleetops.example.com`; single sign-on needs it.

Enterprise license and a sealing key kept outside the volume:

```sh
kubectl -n fleetops create secret generic fleetops-license --from-file=license=fleetops.license
kubectl -n fleetops create secret generic fleetops-secret-key --from-literal=secret-key="$(openssl rand -base64 32)"
helm upgrade --install fleetops fleetops/fleetops -n fleetops \
  --set license.existingSecret=fleetops-license \
  --set secretKey.existingSecret=fleetops-secret-key
```

Set `secretKey.existingSecret` on the first install only, or to the key already in use: a different key cannot open credentials sealed with the old one.

## Values

| Key | Default | Description |
|---|---|---|
| `image.repository` | `docker.io/fleets/fleetops` | Also at `ghcr.io/pravinbanjade/fleetops`. |
| `image.tag` | chart `appVersion` | |
| `image.digest` | `""` | Pins the image by digest instead of tag. |
| `publicUrl` | `""` | Where browsers reach FleetOps; defaults to the first TLS ingress host. |
| `logLevel` | `info` | A tracing filter. |
| `tls.enabled` | `true` | The HTTPS listener agents need. |
| `tls.port` | `9443` | |
| `tls.existingSecret` | `""` | A `kubernetes.io/tls` Secret. Empty: a self-signed certificate for `localhost` and `fleetops`, kept on the volume. |
| `tls.hsts` | `false` | Only when browsers never use plain HTTP. |
| `secretKey.existingSecret` / `.key` | `""` / `secret-key` | The 32-byte base64 key that seals stored credentials. Empty: `/data/secret.key`, created on first start. |
| `license.existingSecret` / `.key` | `""` / `license` | An Enterprise license file. |
| `extraEnv`, `extraEnvFrom` | `[]` | More `FLEETOPS_*` settings, for example `FLEETOPS_KUBE_EXEC_PLUGINS`. |
| `persistence.enabled` | `true` | Off: an `emptyDir`, lost with the pod. |
| `persistence.existingClaim` | `""` | |
| `persistence.storageClass` | `""` | `-` sets `storageClassName: ""`. |
| `persistence.size` | `5Gi` | |
| `persistence.retain` | `true` | Keep the PVC on `helm uninstall`. |
| `service.type` | `ClusterIP` | |
| `service.httpPort` / `.httpsPort` | `9000` / `9443` | |
| `service.httpNodePort` / `.httpsNodePort` | `null` | For `NodePort` and `LoadBalancer`. |
| `ingress.enabled` | `false` | |
| `ingress.backendPort` | `https` | `https` keeps agents working; `http` serves the UI only. |
| `ingress.className`, `.annotations`, `.hosts`, `.tls` | | As usual. |
| `networkPolicy.enabled` | `false` | Allows ingress only from the namespace and `networkPolicy.from`. |
| `resources` | 50m / 64Mi requested, 512Mi limit | |
| `podSecurityContext`, `securityContext` | non-root 65532, read-only root, no capabilities | |
| `nodeSelector`, `tolerations`, `affinity`, `topologySpreadConstraints`, `priorityClassName` | | |
| `extraVolumes`, `extraVolumeMounts` | `[]` | |

## Notes

- **One replica.** FleetOps keeps everything in SQLite on its volume, so the chart runs a single pod and replaces it with the `Recreate` strategy. The volume must be `ReadWriteOnce` (or better) block or file storage, not NFS without locking.
- **No Docker socket.** Kubernetes nodes rarely run Docker, so the chart mounts none and FleetOps starts without a `local` environment. Add Docker hosts through agents.
- **Helm installs from the UI** need a `helm` binary, which the image does not ship. Kubeconfigs with exec plugins (EKS, GKE, AKS) need the plugins, so use the in-cluster agent for those clusters.
- **Backups.** `/data` holds the database, `secret.key` and the generated certificate. Use **Admin → Backup** or snapshot the volume.
