package main

import (
	"testing"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// httpInterface builds a wasi:http host interface advertising the given handler
// interface, optionally with a routing `host` config.
func httpInterface(iface, host string) runtimev1alpha1.HostInterface {
	hi := runtimev1alpha1.HostInterface{
		Namespace:  "wasi",
		Package:    "http",
		Interfaces: []string{iface},
	}
	if host != "" {
		hi.Config = map[string]string{"host": host}
	}
	return hi
}

func TestWorkloadHostname(t *testing.T) {
	tests := []struct {
		name   string
		ifaces []runtimev1alpha1.HostInterface
		want   string
	}{
		{
			// p2 handler entrypoint.
			name:   "incoming-handler is routed",
			ifaces: []runtimev1alpha1.HostInterface{httpInterface("incoming-handler", "a.localhost.direct")},
			want:   "a.localhost.direct",
		},
		{
			// p3 handler entrypoint — the case the gateway previously dropped, so
			// a p3 handler workload reached Ready but every request to it 503'd.
			name:   "handler is routed",
			ifaces: []runtimev1alpha1.HostInterface{httpInterface("handler", "b.localhost.direct")},
			want:   "b.localhost.direct",
		},
		{
			name:   "handler without a host config yields no route",
			ifaces: []runtimev1alpha1.HostInterface{httpInterface("handler", "")},
			want:   "",
		},
		{
			name: "non-http interface is ignored",
			ifaces: []runtimev1alpha1.HostInterface{{
				Namespace:   "acme",
				Package:     "kv",
				Interfaces:  []string{"store"},
				ConfigLayer: runtimev1alpha1.ConfigLayer{Config: map[string]string{"host": "ignored"}},
			}},
			want: "",
		},
		{
			name:   "no interfaces yields no route",
			ifaces: nil,
			want:   "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			w := &runtimev1alpha1.Workload{}
			w.Spec.HostInterfaces = tt.ifaces
			if got := workloadHostname(w); got != tt.want {
				t.Errorf("workloadHostname() = %q, want %q", got, tt.want)
			}
		})
	}
}
