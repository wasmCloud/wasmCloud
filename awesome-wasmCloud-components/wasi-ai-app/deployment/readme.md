# steps for deploying wasi-ai-app in Kubernetes

   $  kubectl delete workloaddeployment wasi3-ai-app
   $  kubectl delete service wasi3-ai-app
   $  k3d cluster delete wasmcloud

   $ k3d cluster create wasmcloud --port "80:30950@server:0"

   $ helm install wasmcloud --version 2.5.2 oci://ghcr.io/wasmcloud/charts/runtime-operator \
     --namespace wasmcloud --create-namespace \
     -f /deployment/values.local.yaml
   
   $ kubectl get pods -l app.kubernetes.io/instance=wasmcloud -n wasmcloud
   
   $ kubectl rollout status deploy -l app.kubernetes.io/instance=wasmcloud -n wasmcloud
   
   $ kubectl apply -f /home/b/me/WebAssembly/wasmCloud/p3/wasi-ai-app/deployment/deployment.yaml
   
   $ kubectl get workloaddeployments

   $ kubectl get endpoints wasi3-ai-app -n wasmcloud

   $ curl -v localhost
   
   $ kubectl logs -n wasmcloud hostgroup-default-5d4f7ff59-45hns

   $ kubectl describe pod -n wasmcloud hostgroup-default-7596596c59-px56t | grep -A5 "Args:"
   $ kubectl get hosts -n wasmcloud
   
   $ kubectl describe host -n wasmcloud dangerous-meal-6408

   ---make required data/model files into /mnt/data from /testdata

   $ kubectl apply -f /deployment/temp-copier.yaml

   $ kubectl cp /testdata/. wasmcloud/temp-copier:/mnt/data/

   $ kubectl exec -it -n wasmcloud hostgroup-default-7596596c59-px56t -- ls -la /var/data

   $ kubectl exec -it -n wasmcloud temp-copier -- ls -la /mnt/data

   $ kubectl delete pod temp-copier -n wasmcloud

   ---
   
   $ kubectl get pod -n wasmcloud hostgroup-default-7596596c59-px56t -o jsonpath='{.spec.containers[0].image}'

  ---
  
   $ ffmpeg -i podcast-rust-niko_2min.wav -t 60 -c copy podcast-rust-niko_1min.wav


