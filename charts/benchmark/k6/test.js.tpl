import http from "k6/http";

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

// NOTE: In the future we could use this function to do something like store a configmap with the
// output of the test. In conjunction with a custom pod we could watch the configmap objects
// filtered by the labeled configmap and then run a script to generate an aggregated report.
// export function handleSummary(data) {
//   return {
//     'summary.json': JSON.stringify(data), //the default data object
//   };
// }

const url = "{{ .Values.test.url }}";
export default function () {
  http.get(url);
}