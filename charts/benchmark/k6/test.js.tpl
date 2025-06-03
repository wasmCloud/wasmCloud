import http from "k6/http";
import { Kubernetes } from 'k6/x/kubernetes';
import { textSummary } from 'https://jslib.k6.io/k6-summary/0.1.0/index.js';

export const options = {
  discardResponseBodies: true,
  scenarios: {
    {{- range $name, $scenario := .Values.test.scenarios -}}
    "{{ $name }}": {{ toJson $scenario }},
    {{- end }}
  },
};

function annotate(title, data) {
  http.post(
    "http://{{ .Release.Name }}-grafana/api/annotations/graphite",
    JSON.stringify({
      what: title,
      tags: ["deploy"],
      when: Math.round(new Date().getTime() / 1000),
      data: data,
    }),
    {
      headers: {
        "Content-Type": "application/json",
      },
    },
  );
}

export function setup() {
  annotate("Test start", "wasmCloud Benchmark Chart");
}

export function teardown() {
  annotate("Test end", "wasmCloud Benchmark Chart");
}

var configMap = {
  apiVersion: "v1",
  kind: "ConfigMap",
  metadata: {
    name: `${__ENV.MY_POD_NAME}`,
    namespace: "{{ .Release.Namespace }}",
    labels: {
      "k6-result": "true",
      "k6-test-name": "{{ include "benchmark.fullname" . }}-test",
      "chart-revision": `{{ .Release.Revision }}`
    }
  },
  data: {}
};

// Writes out the results to a configmap for later aggregation/consumption
export function handleSummary(data) {
  configMap.data.results = JSON.stringify(data);
  const kubernetes = new Kubernetes();
  // Don't know why, but we have to stringify the configmap first even though the k6 examples don't
  // show this
  kubernetes.apply(JSON.stringify(configMap));
  return {
    stdout: textSummary(data, { enableColors: true }) + "\n\n",
  };
}

const url = {{ required "A test URL must be set" .Values.test.url | quote }};
export default function () {
  http.get(url);
}