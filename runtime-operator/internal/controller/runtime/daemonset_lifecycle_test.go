package runtime

import (
	"context"
	"errors"
	"reflect"
	"strings"
	"testing"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	daemonSetWorkloadResource = "workloads"
	artifactPendingCase       = "artifact condition pending"
)

func TestDaemonSetScheduleWaitsForArtifacts(t *testing.T) {
	for _, state := range []string{artifactPendingCase, "component artifact missing"} {
		t.Run(state, func(t *testing.T) {
			ds := testDaemonSet(1)
			if state == artifactPendingCase {
				ds.Status.Conditions = nil
			} else {
				ds.Spec.Template.Spec.Components[0].Image = "artifact://missing"
			}
			r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA))
			before := ds.DeepCopy()
			err := r.reconcileSchedule(t.Context(), ds)
			want := "artifact missing not found in deployment spec"
			if state == artifactPendingCase {
				want = "artifacts not ready"
			}
			if err == nil || !strings.Contains(err.Error(), want) || condition.IsStatusUnknown(err) != (state == artifactPendingCase) {
				t.Fatalf("schedule = %v, want %q", err, want)
			}
			if !reflect.DeepEqual(ds, before) || len(workloadsByHost(t, r)) != 0 {
				t.Fatal("blocked schedule changed state")
			}
		})
	}
}

func TestDaemonSetReadyWaitsForArtifactsAndSchedule(t *testing.T) {
	for _, ready := range []condition.ConditionType{
		runtimev1alpha1.WorkloadDaemonSetConditionArtifact,
		runtimev1alpha1.WorkloadDaemonSetConditionSchedule,
	} {
		t.Run(string(ready)+" only", func(t *testing.T) {
			ds := testDaemonSet(1)
			ds.Status.Conditions = nil
			ds.Status.SetConditions(condition.ReadyCondition(ready))
			err := (&WorkloadDaemonSetReconciler{}).reconcileReady(t.Context(), ds)
			if !condition.IsStatusUnknown(err) || !strings.Contains(err.Error(), "daemonset is not scheduled") {
				t.Fatalf("ready = %v, want not scheduled", err)
			}
		})
	}
}

func TestDaemonSetDefaultReplicasAndService(t *testing.T) {
	ds := testDaemonSet(1)
	ds.Spec.ReplicasPerHost = nil
	ds.Spec.Kubernetes = &runtimev1alpha1.KubernetesSpec{Service: &runtimev1alpha1.KubernetesServiceRef{Name: "echo"}}
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA))
	for range 2 {
		schedule(t, r, ds)
	}
	byHost := workloadsByHost(t, r)
	assertPerHost(t, byHost, map[string]int{dsHostA: 1})
	if !reflect.DeepEqual(byHost[dsHostA][0].Spec.Kubernetes, ds.Spec.Kubernetes) || ds.Spec.Template.Spec.Kubernetes != nil {
		t.Fatal("service was not copied without changing the template")
	}
	if ds.Status.Replicas.Expected != 1 {
		t.Fatalf("default expected replicas = %d", ds.Status.Replicas.Expected)
	}
}

func TestDaemonSetReadFailuresStartNothingAndRetry(t *testing.T) {
	for _, operation := range []string{"host list failure", "owned workloads", "orphans", "confirm owner"} {
		t.Run(operation, func(t *testing.T) {
			ds := testDaemonSet(1)
			hash := ds.Spec.Template.Hash()
			orphan := orphanedWorkload(daemonSetWorkloadName(ds.Name, dsHostA, hash, 0), dsHostA, hash)
			r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), orphan)
			cause := errors.New("API unavailable")
			fail := true
			c := interceptor.NewClient(r.Client.(client.WithWatch), interceptor.Funcs{
				List: func(ctx context.Context, c client.WithWatch, list client.ObjectList, opts ...client.ListOption) error {
					query := (&client.ListOptions{}).ApplyOptions(opts)
					_, hosts := list.(*runtimev1alpha1.HostList)
					_, workloads := list.(*runtimev1alpha1.WorkloadList)
					if fail && (operation == "host list failure" && hosts || operation == "owned workloads" && workloads && query.FieldSelector != nil || operation == "orphans" && workloads && query.FieldSelector == nil) {
						return cause
					}
					return c.List(ctx, list, opts...)
				},
				Get: func(ctx context.Context, c client.WithWatch, key client.ObjectKey, obj client.Object, opts ...client.GetOption) error {
					if fail && operation == "confirm owner" {
						return cause
					}
					return c.Get(ctx, key, obj, opts...)
				},
			})
			r.Client, r.APIReader = c, c
			if err := r.reconcileSchedule(t.Context(), ds); !errors.Is(err, cause) {
				t.Fatalf("schedule = %v, want API error", err)
			}
			fail = false
			got := &runtimev1alpha1.Workload{}
			if err := c.Get(t.Context(), client.ObjectKeyFromObject(orphan), got); err != nil || metav1.GetControllerOf(got) != nil {
				t.Fatalf("failed read adopted orphan: %+v, %v", got.OwnerReferences, err)
			}
			for range 2 {
				schedule(t, r, ds)
			}
			byHost := workloadsByHost(t, r)
			assertPerHost(t, byHost, map[string]int{dsHostA: 1})
			if byHost[dsHostA][0].Name != orphan.Name {
				t.Fatal("retry replaced the orphan instead of adopting it")
			}
		})
	}
}

func TestDaemonSetAdoptionPatchFailures(t *testing.T) {
	for _, reason := range []string{"orphan disappeared", "adoption unavailable"} {
		t.Run(reason, func(t *testing.T) {
			ds := testDaemonSet(1)
			hash := ds.Spec.Template.Hash()
			orphan := orphanedWorkload(daemonSetWorkloadName(ds.Name, dsHostA, hash, 0), dsHostA, hash)
			r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), orphan)
			cause := errors.New("API unavailable")
			fail := true
			r.Client = interceptor.NewClient(r.Client.(client.WithWatch), interceptor.Funcs{
				Patch: func(ctx context.Context, c client.WithWatch, obj client.Object, patch client.Patch, opts ...client.PatchOption) error {
					if fail {
						if reason == "orphan disappeared" {
							if err := c.Delete(ctx, obj); err != nil {
								return err
							}
							return apierrors.NewNotFound(schema.GroupResource{Resource: daemonSetWorkloadResource}, obj.GetName())
						}
						return cause
					}
					return c.Patch(ctx, obj, patch, opts...)
				},
			})
			err := r.reconcileSchedule(t.Context(), ds)
			if reason == "adoption unavailable" {
				if !errors.Is(err, cause) || len(workloadsByHost(t, r)) != 0 {
					t.Fatalf("failed adoption = %v", err)
				}
			} else if err != nil {
				t.Fatal(err)
			}
			fail = false
			for range 2 {
				schedule(t, r, ds)
			}
			assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1})
		})
	}
}

func TestDaemonSetLongNamesPreserveSlots(t *testing.T) {
	suffix := "-" + shortHash(dsHostA) + "-template-0"
	name := strings.Repeat("a", 253-len(suffix)-1) + "." + strings.Repeat("b", len(suffix))
	first := daemonSetWorkloadName(name, dsHostA, "template", 0)
	second := daemonSetWorkloadName(name, dsHostA, "template", 1)
	if len(first) > 253 || strings.Contains(first, ".-") || first == second || first != daemonSetWorkloadName(name, dsHostA, "template", 0) {
		t.Fatalf("slot names are invalid or unstable: %q, %q", first, second)
	}
}
