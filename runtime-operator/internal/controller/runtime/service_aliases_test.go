package runtime

import (
	"testing"

	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
)

// TestServiceDNSAliasesReachEveryIncomingHandler checks that a Service's names
// reach a workload's incoming HTTP handler, WASI 0.2's `incoming-handler` and
// WASI 0.3's `handler` alike, and nothing else.
func TestServiceDNSAliasesReachEveryIncomingHandler(t *testing.T) {
	cases := []struct {
		name      string
		iface     *runtimev2.WitInterface
		wantAlias bool
	}{
		{"wasi 0.2 incoming handler", &runtimev2.WitInterface{Namespace: "wasi", Package: "http", Interfaces: []string{"incoming-handler"}}, true},
		{"wasi 0.3 handler", &runtimev2.WitInterface{Namespace: "wasi", Package: "http", Interfaces: []string{"handler"}}, true},
		{"outgoing only", &runtimev2.WitInterface{Namespace: "wasi", Package: "http", Interfaces: []string{"outgoing-handler"}}, false},
		{"another package", &runtimev2.WitInterface{Namespace: "wasmcloud", Package: "messaging", Interfaces: []string{"handler"}}, false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			interfaces := []*runtimev2.WitInterface{c.iface}
			injectServiceDNSAliases(interfaces, "svc", "ns")
			got := interfaces[0].GetConfig()["host-aliases"]
			want := ""
			if c.wantAlias {
				want = "svc.ns,svc.ns.svc"
			}
			if got != want {
				t.Errorf("host-aliases = %q, want %q", got, want)
			}
		})
	}
}
