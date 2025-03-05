#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(dirname "$0")"

source "$SCRIPT_DIR/common-env"

# NOTE(thomastaylor312): We have to specify the image ID, so this is the command you can use to the
# find the latest images and select the one compatible with the k8s version
# oci ce node-pool-options get --compartment-id "$COMPARTMENT_ID" --node-pool-option-id all
: ${OCI_MANAGED_NODE_IMAGE_ID:="ocid1.image.oc1.iad.aaaaaaaarkexs7ijdqffxvy6dyzepizvuzm25zijwp2nkv3nifbcvywzufsq"}
export OCI_MANAGED_NODE_IMAGE_ID

# NOTE(thomastaylor312): We do this in stages so we can patch the default networking options for the
# node pools. Otherwise we have to have some brittle yaml that assumes the default networking
# options created by the cluster API operator don't change
KUBERNETES_VERSION=v1.31.1 \
NAMESPACE=default \
clusterctl generate cluster benchmark-test \
--from "${SCRIPT_DIR}/manifests/machine-pools.yaml" | kubectl apply --filename -

# Now patch
kubectl patch ocimanagedmachinepools default-node-pool --type merge --patch-file "${SCRIPT_DIR}/manifests/patch.yaml"

# Now generate the cluster
KUBERNETES_VERSION=v1.31.1 \
NAMESPACE=default \
clusterctl generate cluster benchmark-test \
--from "${SCRIPT_DIR}/manifests/cluster.yaml" | kubectl apply --filename -

echo "Waiting for cluster to be ready"
# Wait for cluster to be ready. If the machine pools are up, the cluster is up
kubectl wait -n default --timeout=1h --for=jsonpath='{.status.phase}'=Running machinepools --selector 'cluster.x-k8s.io/cluster-name=benchmark-test'

CLUSTER_ID=$(kubectl get ocimanagedcontrolplane benchmark-test -n default -o jsonpath='{.spec.id}')

oci ce cluster create-kubeconfig --cluster-id "$CLUSTER_ID" --region "$OCI_REGION" --file kubeconfig.yaml --overwrite --kube-endpoint PUBLIC_ENDPOINT

export KUBECONFIG="kubeconfig.yaml"

echo "Waiting for nodes to be ready"

# Give the nodes a bit of extra time to come up and stabilize. Otherwise the next steps fail about
# 25% of the time for some reason
sleep 30

# Double check all nodes are ready
kubectl wait --for=condition=Ready nodes --all --timeout=10m

# Manually taint the nodes since we can't do that in config
kubectl taint nodes --overwrite --selector "pool-name=wasmcloud-pool" pool=wasmcloud-pool:NoSchedule
kubectl taint nodes --overwrite --selector "pool-name=k6-pool" pool=k6-pool:NoSchedule
kubectl taint nodes --overwrite --selector "pool-name=nats-pool" pool=nats-pool:NoSchedule

echo "Kubernetes cluster setup complete. Deploying wasmcloud"
kubectl kustomize "${SCRIPT_DIR}/manifests/wasmcloud/" | kubectl apply --filename -

echo "Waiting for wasmcloud pods to be ready"
# Wait on all testbed pods to be ready
kubectl -n testbed wait --timeout=10m --for=jsonpath='{.status.phase}'=Running --all pods

# Port forward to the nats server
cleanup() {
    echo "Cleaning up"
    local pids=$(jobs -pr)
    [ -n "$pids" ] && kill $pids || true

    unset KUBECONFIG
    rm -f kubeconfig.yaml
    $SCRIPT_DIR/delete-cluster.sh
}

trap "cleanup" INT QUIT TERM EXIT ERR

echo "Starting NATS Proxy"
$SCRIPT_DIR/nats-proxy.sh 2>&1 >/dev/null &

echo "Waiting for NATS connection"
while ! nc -z localhost 4222; do
  echo "checking NATS..."
  sleep 1
done

echo "Waiting for host & wadm to be ready"
while ! wash get claims 2>&1 >/dev/null; do
  echo "checking host..."
  sleep 1
done

while ! wash app list 2>&1 >/dev/null; do
  echo "checking wadm..."
  sleep 1
done

echo "Deploying application"
wash app deploy "${SCRIPT_DIR}/manifests/wadm.yaml"

timeout 120s bash -c 'while true; do
  status=$(wash app status http -o json | jq -r ".status.status.type")
  if [ "$status" = "deployed" ]; then
    break
  fi
  echo "checking for app deployment..."
  sleep 2
done'

echo "Application deployed, ensuring all nodes are ready"

# The OCI nodes take a while to stabilize and keep thrashing between ready and not ready. This lets
# us at least attempt to avoid that by ensuring the nodes are ready before the helm install. We do
# check this above, but since there is thrashing, we need to check again 
kubectl wait --for=condition=Ready nodes --all --timeout=10m

helm upgrade --install my-benchmark --namespace testbed --version 0.2.1 oci://ghcr.io/wasmcloud/charts/benchmark --wait --values "$SCRIPT_DIR/values.yaml"

echo "Waiting for test run to complete"
kubectl wait --namespace testbed --timeout=15m --for=jsonpath='{.status.stage}'=finished testruns/my-benchmark-test

# Give it an extra second to make sure everything was written out to the config map
sleep 1

echo "Collecting and printing results"
kubectl get cm --namespace testbed --output json --selector 'k6-result=true,k6-test-name=my-benchmark-test' | jq  '[.items[].data.results | fromjson ]' | "$SCRIPT_DIR/aggregate_data.py"
