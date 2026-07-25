package runtime

import (
	"context"
	"testing"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// The hostInterfaces routing invariants are enforced by CEL XValidation rules on
// WorkloadSpec — string expressions the Go compiler cannot check, so they are
// exercised here against a real API server.
//
// The rules answer one question: can the host tell these bindings apart? An
// import resolves by its externalId when it declares one and by its name
// otherwise, and a binding may pin either (or neither, making it the package's
// default route). Bindings that nothing could distinguish must be refused here
// rather than discovered when a workload fails to bind.
//
// External-ids are not identity — the component model puts no uniqueness
// requirement on them — so these rules constrain the operator's own config, not
// what an artifact is allowed to declare.

func kvInterface(name, externalID string) runtimev1alpha1.HostInterface {
	return runtimev1alpha1.HostInterface{
		Name:       name,
		ExternalID: externalID,
		Namespace:  "wasi",
		Package:    "keyvalue",
		Version:    "0.2.0-draft",
		Interfaces: []string{"store"},
		ConfigLayer: runtimev1alpha1.ConfigLayer{
			Config: map[string]string{"backend": "in-memory"},
		},
	}
}

func createWorkload(
	ctx context.Context,
	c client.Client,
	ns, name string,
	ifaces ...runtimev1alpha1.HostInterface,
) error {
	return c.Create(ctx, &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: ns},
		Spec: runtimev1alpha1.WorkloadSpec{
			HostInterfaces: ifaces,
		},
	})
}

func TestHostInterfacesAdmission(t *testing.T) {
	c, ctx := startHostEnvtest(t)
	ns := createTestNamespace(t, ctx, c, "hostinterfaces-admission")

	cases := []struct {
		name    string
		ifaces  []runtimev1alpha1.HostInterface
		allowed bool
		why     string
	}{
		{
			name: "distinct external ids are two bindings",
			ifaces: []runtimev1alpha1.HostInterface{
				kvInterface("", "user-db-prod:region-a"),
				kvInterface("", "catalog-db-prod:region-a"),
			},
			allowed: true,
			why:     "neither is named, but they serve different platform resources",
		},
		{
			name: "the same external id twice is one resource claimed twice",
			ifaces: []runtimev1alpha1.HostInterface{
				kvInterface("", "user-db-prod:region-a"),
				kvInterface("", "user-db-prod:region-a"),
			},
			allowed: false,
			why:     "duplicates in every identifying field",
		},
		{
			name: "a default route coexists with a resource-keyed binding",
			ifaces: []runtimev1alpha1.HostInterface{
				kvInterface("", ""),
				kvInterface("", "user-db-prod:region-a"),
			},
			allowed: true,
			why:     "the unkeyed entry is the fallback, the keyed one outranks it",
		},
		{
			name: "two unkeyed entries are two defaults",
			ifaces: []runtimev1alpha1.HostInterface{
				kvInterface("", ""),
				kvInterface("", ""),
			},
			allowed: false,
			why:     "the default route cannot be shared",
		},
		{
			name: "a name pin coexists with a resource binding",
			ifaces: []runtimev1alpha1.HostInterface{
				kvInterface("users", ""),
				kvInterface("", "user-db-prod:region-a"),
			},
			allowed: true,
			why: "an import matching both resolves to the name pin, since " +
				"deploy-time config outranks what the artifact declares",
		},
		{
			name: "declaring both keys narrows rather than duplicates",
			ifaces: []runtimev1alpha1.HostInterface{
				kvInterface("users", "user-db-prod:region-a"),
				kvInterface("users", "user-db-staging"),
			},
			allowed: true,
			why:     "same name, different resource — distinct bindings",
		},
		{
			name: "an exact duplicate is never admissible",
			ifaces: []runtimev1alpha1.HostInterface{
				kvInterface("users", "user-db-prod:region-a"),
				kvInterface("users", "user-db-prod:region-a"),
			},
			allowed: false,
			why:     "identical in every identifying field",
		},
	}

	for i, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			name := workloadName(i)
			err := createWorkload(ctx, c, ns, name, tc.ifaces...)
			switch {
			case tc.allowed && err != nil:
				t.Fatalf("expected admission to accept (%s): %v", tc.why, err)
			case !tc.allowed && err == nil:
				t.Fatalf("expected admission to reject (%s), but it was created", tc.why)
			}
			if err == nil {
				_ = c.Delete(ctx, &runtimev1alpha1.Workload{
					ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: ns},
				})
			}
		})
	}
}

// workloadName keeps each case's object name unique and DNS-safe without
// depending on the case description.
func workloadName(i int) string {
	return "admission-case-" + string(rune('a'+i))
}
