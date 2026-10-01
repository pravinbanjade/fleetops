#!/bin/sh
# The agent's Kubernetes mode end to end, with the real images: the server
# runs on kind's Docker network with TLS (its self-signed cert names
# "fleetops"), the generated manifest is applied to the kind cluster, and
# the environment must come up and list the agent's own Deployment.
#
# Usage: scripts/agent-kube-smoke.sh [server image] [agent image]
set -eu

server_image=${1:-fleetops:dev}
agent_image=${2:-fleetops-agent:dev}
cluster=${KIND_CLUSTER:-fleetops-test}
srv=fleetops-kube-smoke-$$
work=$(mktemp -d)
jar="$work/jar"
export KUBECONFIG="$work/kubeconfig"

cleanup() {
  docker rm -f "$srv" >/dev/null 2>&1 || true
  kubectl delete namespace fleetops --ignore-not-found --wait=false >/dev/null 2>&1 || true
  kubectl delete clusterrolebinding fleetops-agent --ignore-not-found >/dev/null 2>&1 || true
  kubectl delete clusterrole fleetops-agent --ignore-not-found >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

fail() {
  echo "agent-kube-smoke: $*" >&2
  docker logs "$srv" >&2 2>&1 || true
  kubectl -n fleetops logs deploy/fleetops-agent >&2 2>&1 || true
  kubectl -n fleetops get events --sort-by=.lastTimestamp >&2 2>&1 | tail -8 || true
  exit 1
}

kind get clusters 2>/dev/null | grep -qx "$cluster" || fail "no kind cluster '$cluster': run make kind-up"
kind get kubeconfig --name "$cluster" > "$KUBECONFIG"
# A run that was killed leaves its namespace behind.
kubectl delete namespace fleetops --ignore-not-found --wait=true >/dev/null
# `kind load` cannot read multi-platform images from Docker's containerd
# store; importing the node's own platform works with either store.
node="$cluster-control-plane"
plat="linux/$(docker exec "$node" uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')"
docker save --platform "$plat" "$agent_image" |
  docker exec -i "$node" ctr --namespace=k8s.io images import --digests --snapshotter=overlayfs - >/dev/null ||
  fail "could not load $agent_image into the cluster"

docker run -d --name "$srv" --network kind -p 127.0.0.1::9443 "$server_image" --tls-bind 0.0.0.0:9443 >/dev/null
ip=$(docker inspect -f '{{(index .NetworkSettings.Networks "kind").IPAddress}}' "$srv")
base="https://$(docker port "$srv" 9443/tcp | head -n1)"
i=0
until curl -fsSk "$base/api/v1/health" >/dev/null 2>&1; do
  i=$((i + 1)); [ "$i" -lt 30 ] || fail "server did not start"; sleep 1
done
docker cp "$srv:/data/tls/cert.pem" "$work/ca.pem" || fail "no server certificate"

origin="https://fleetops:9443"
csrf=$(curl -fsSk -c "$jar" -H 'content-type: application/json' -H "Origin: $base" \
  -d '{"username":"admin","password":"smoke-password-1"}' "$base/api/v1/auth/setup" |
  sed -n 's/.*"csrf_token":"\([^"]*\)".*/\1/p')
[ -n "$csrf" ] || fail "setup"
curl -fsSk -b "$jar" -H 'content-type: application/json' -H "Origin: $origin" -H "x-csrf-token: $csrf" \
  -d '{"name":"cluster","kind":"kube-agent"}' "$base/api/v1/envs" > "$work/env.json" || fail "create env"
python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["enrollment"]["command"])' "$work/env.json" |
  sed "s#docker.io/pravinbanjade/fleetops-agent:[^ ]*#$agent_image#" > "$work/apply.sh"
grep -q -- '--kube' "$work/apply.sh" || fail "command is not a kube manifest"
sh "$work/apply.sh" >/dev/null || fail "kubectl apply"

# Trust the self-signed server and resolve its name to the container.
kubectl -n fleetops create configmap fleetops-ca --from-file=ca.pem="$work/ca.pem" >/dev/null || fail "CA configmap"
kubectl -n fleetops patch deployment fleetops-agent --type json -p "[
  {\"op\":\"add\",\"path\":\"/spec/template/spec/hostAliases\",\"value\":[{\"ip\":\"$ip\",\"hostnames\":[\"fleetops\"]}]},
  {\"op\":\"add\",\"path\":\"/spec/template/spec/volumes\",\"value\":[{\"name\":\"ca\",\"configMap\":{\"name\":\"fleetops-ca\"}}]},
  {\"op\":\"add\",\"path\":\"/spec/template/spec/containers/0/volumeMounts\",\"value\":[{\"name\":\"ca\",\"mountPath\":\"/ca\"}]},
  {\"op\":\"add\",\"path\":\"/spec/template/spec/containers/0/args/-\",\"value\":\"--ca\"},
  {\"op\":\"add\",\"path\":\"/spec/template/spec/containers/0/args/-\",\"value\":\"/ca/ca.pem\"}
]" >/dev/null || fail "patch"

i=0
until curl -fsSk -b "$jar" "$base/api/v1/envs/cluster/status" | grep -q '"state":"up"'; do
  i=$((i + 1)); [ "$i" -lt 90 ] || fail "the cluster environment did not come up"; sleep 2
done
curl -fsSk -b "$jar" "$base/api/v1/envs/cluster/k8s/deployments?namespace=fleetops" | grep -q '"name":"fleetops-agent"' ||
  fail "the agent's Deployment is not listed through the tunnel"
kubectl -n fleetops get secret fleetops-agent-key >/dev/null || fail "the agent did not keep its key in its Secret"
echo "agent-kube-smoke: ok"
