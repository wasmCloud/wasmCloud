package runtime

import (
	"testing"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// An owner index files a child only under the parent that controls it: a
// plain owner, or a controller of another group or kind, is not that parent.
func TestControllerUID(t *testing.T) {
	gvk := runtimev1alpha1.GroupVersion.WithKind("WorkloadReplicaSet")
	ref := func(apiVersion, kind string, uid types.UID, controller bool) metav1.OwnerReference {
		return metav1.OwnerReference{APIVersion: apiVersion, Kind: kind, Name: "parent", UID: uid, Controller: new(controller)}
	}
	ours := runtimev1alpha1.GroupVersion.String()
	for _, tc := range []struct {
		name string
		refs []metav1.OwnerReference
		want string
	}{
		{"controller", []metav1.OwnerReference{ref(ours, gvk.Kind, "rs", true)}, "rs"},
		{"plain owner", []metav1.OwnerReference{ref(ours, gvk.Kind, "rs", false)}, ""},
		{"owner without a controller flag", []metav1.OwnerReference{{APIVersion: ours, Kind: gvk.Kind, Name: "parent", UID: "rs"}}, ""},
		{"controller in another group", []metav1.OwnerReference{ref("apps/v1", gvk.Kind, "rs", true)}, ""},
		{"controller of another kind", []metav1.OwnerReference{ref(ours, "WorkloadDeployment", "d", true)}, ""},
		{"controller after a plain owner", []metav1.OwnerReference{ref(ours, gvk.Kind, "old", false), ref(ours, gvk.Kind, "rs", true)}, "rs"},
	} {
		obj := &runtimev1alpha1.Workload{ObjectMeta: metav1.ObjectMeta{OwnerReferences: tc.refs}}
		got, ok := controllerUID(obj, gvk)
		if got != tc.want || ok != (tc.want != "") {
			t.Errorf("%s: controllerUID = %q, %v; want %q", tc.name, got, ok, tc.want)
		}
	}
}
