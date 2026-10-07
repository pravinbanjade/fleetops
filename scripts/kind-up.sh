#!/bin/sh
# Creates (or reuses) the kind cluster the Kubernetes tests run against and
# writes what they need into $1 (default target/kube-it): kubeconfig,
# server, ca.crt, token (cluster-admin) and narrow-token (pods in
# fleetops-it only).
set -eu
name=${KIND_CLUSTER:-fleetops-test}
out=${1:-target/kube-it}
for tool in kind kubectl docker; do
  command -v "$tool" >/dev/null || { echo "$tool is required (kind: brew install kind)" >&2; exit 1; }
done
kind get clusters 2>/dev/null | grep -qx "$name" || kind create cluster --name "$name" --wait 120s
mkdir -p "$out"
kind get kubeconfig --name "$name" > "$out/kubeconfig"
export KUBECONFIG="$out/kubeconfig"
# Preloading saves a pull. `kind load` cannot read multi-platform images
# from Docker's containerd store, so import the node's own platform; if
# that fails too, the cluster pulls instead.
node="$name-control-plane"
plat="linux/$(docker exec "$node" uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')"
docker pull -q --platform "$plat" busybox:1.37 >/dev/null
docker save --platform "$plat" busybox:1.37 |
  docker exec -i "$node" ctr --namespace=k8s.io images import --digests --snapshotter=overlayfs - >/dev/null 2>&1 ||
  echo "busybox preload failed; the cluster will pull it"
kubectl apply -f "$(dirname "$0")/kube-it-fixtures.yaml" >/dev/null
kubectl wait --for=condition=Established crd/widgets.example.fleetops.io --timeout=60s >/dev/null
kubectl -n fleetops-it rollout status deploy/web --timeout=120s >/dev/null
kubectl -n fleetops-it wait --for=condition=Ready pod/ticker --timeout=120s >/dev/null
kubectl -n fleetops-it create token fleetops-it --duration=2h > "$out/token"
kubectl -n fleetops-it create token fleetops-it-narrow --duration=2h > "$out/narrow-token"
kubectl config view --raw -o jsonpath='{.clusters[0].cluster.certificate-authority-data}' | base64 -d > "$out/ca.crt"
kubectl config view --raw -o jsonpath='{.clusters[0].cluster.server}' > "$out/server"
echo "kind cluster $name ready; fixtures in $out"
