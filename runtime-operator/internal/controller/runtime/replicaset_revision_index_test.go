package runtime

import (
	"fmt"
	"testing"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/cache"
	"sigs.k8s.io/controller-runtime/pkg/client"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func BenchmarkReplicaSetRevisionLookup(b *testing.B) {
	const template = "shared-template"
	const target = "replicaset-2500"
	gvk := runtimev1alpha1.GroupVersion.WithKind("WorkloadReplicaSet")
	revisionIndex := replicaSetRevisionIndexValue(gvk)
	indexer := cache.NewIndexer(cache.MetaNamespaceKeyFunc, cache.Indexers{
		"template": func(object any) ([]string, error) {
			return []string{object.(client.Object).GetAnnotations()[workloadReplicaSetGenerationAnnotation]}, nil
		},
		"revision": func(object any) ([]string, error) {
			return revisionIndex(object.(client.Object)), nil
		},
	})
	controller := true
	for i := range 5000 {
		workload := &runtimev1alpha1.Workload{ObjectMeta: metav1.ObjectMeta{
			Name:        fmt.Sprintf("workload-%d", i),
			Namespace:   "benchmark",
			Annotations: map[string]string{workloadReplicaSetGenerationAnnotation: template},
			OwnerReferences: []metav1.OwnerReference{{
				APIVersion: gvk.GroupVersion().String(), Kind: gvk.Kind,
				UID: types.UID(fmt.Sprintf("replicaset-%d", i)), Controller: &controller,
			}},
		}}
		if err := indexer.Add(workload); err != nil {
			b.Fatal(err)
		}
	}
	b.Run("shared-template-first", func(b *testing.B) {
		b.ReportAllocs()
		for b.Loop() {
			candidates, err := indexer.ByIndex("template", template)
			if err != nil {
				b.Fatal(err)
			}
			matches := 0
			for _, candidate := range candidates {
				if uid, owned := controllerUID(candidate.(client.Object), gvk); owned && uid == target {
					matches++
				}
			}
			if matches != 1 {
				b.Fatalf("matched %d workloads, want one", matches)
			}
		}
	})
	b.Run("owner-and-revision", func(b *testing.B) {
		key := replicaSetRevisionKey(target, template)
		b.ReportAllocs()
		for b.Loop() {
			candidates, err := indexer.ByIndex("revision", key)
			if err != nil {
				b.Fatal(err)
			}
			if len(candidates) != 1 {
				b.Fatalf("matched %d workloads, want one", len(candidates))
			}
		}
	})
}
