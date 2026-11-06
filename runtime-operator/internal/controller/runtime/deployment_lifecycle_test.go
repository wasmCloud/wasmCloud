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
	"k8s.io/client-go/tools/events"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func TestDeploymentWaitsForRolloutPrerequisites(t *testing.T) {
	for _, state := range []string{"artifacts pending", "already synced", "scale pending", "missing current reference", "missing current ReplicaSet"} {
		t.Run(state, func(t *testing.T) {
			d := readyDeployment("")
			r := newScaleReconciler(t)
			before := d.DeepCopy()
			var err error
			switch state {
			case "artifacts pending":
				err = r.reconcileDeploy(t.Context(), d)
				if !condition.IsStatusUnknown(err) {
					t.Fatalf("deploy = %v, want Unknown", err)
				}
			case "already synced":
				d.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadDeploymentConditionArtifact))
				before = d.DeepCopy()
				if err = r.reconcileDeploy(t.Context(), d); err != nil {
					t.Fatal(err)
				}
			case "scale pending":
				d.Status.Conditions = nil
				before = d.DeepCopy()
				if err = r.reconcileScale(t.Context(), d); !condition.IsStatusUnknown(err) {
					t.Fatalf("scale = %v, want Unknown", err)
				}
			case "missing current reference":
				d.Status.CurrentReplicaSet = nil
				before = d.DeepCopy()
				if err = r.reconcileScale(t.Context(), d); err == nil || !strings.Contains(err.Error(), "current ReplicaSet is nil") {
					t.Fatalf("scale = %v, want missing reference", err)
				}
			case "missing current ReplicaSet":
				if err = r.reconcileScale(t.Context(), d); !apierrors.IsNotFound(err) {
					t.Fatalf("scale = %v, want NotFound", err)
				}
			}
			if !reflect.DeepEqual(d, before) {
				t.Fatal("blocked rollout changed the deployment")
			}
			list := &runtimev1alpha1.WorkloadReplicaSetList{}
			if err := r.List(t.Context(), list); err != nil || len(list.Items) != 0 {
				t.Fatalf("blocked rollout created ReplicaSets: %+v, %v", list.Items, err)
			}
		})
	}
}

func TestDeploymentScaleFailuresPreserveStateAndRetry(t *testing.T) {
	const (
		previousAbsentCase = "previous already gone"
		updateDeniedCase   = "update denied"
	)
	for _, operation := range []string{testListOperation, updateDeniedCase, "update unavailable", "delete stale", "previous ReplicaSet delete failure", previousAbsentCase} {
		t.Run(operation, func(t *testing.T) {
			d := readyDeployment(testPrevSetName)
			current := availableReplicaSet(testReplicaSetName)
			previous := availableReplicaSet(testPrevSetName)
			stale := availableReplicaSet("stale")
			if strings.HasPrefix(operation, "update") {
				d.Spec.Replicas = new(int32(2))
			}
			cause := errors.New("API unavailable")
			if operation == updateDeniedCase {
				cause = apierrors.NewForbidden(schema.GroupResource{Resource: "workloadreplicasets"}, current.Name, errors.New("policy denied"))
			}
			fail := true
			funcs := interceptor.Funcs{
				List: func(ctx context.Context, c client.WithWatch, list client.ObjectList, opts ...client.ListOption) error {
					if fail && operation == testListOperation {
						return cause
					}
					return c.List(ctx, list, opts...)
				},
				Update: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.UpdateOption) error {
					if fail && strings.HasPrefix(operation, "update") {
						return cause
					}
					return c.Update(ctx, obj, opts...)
				},
				Delete: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.DeleteOption) error {
					if fail && (operation == "delete stale" && obj.GetName() == stale.Name || operation == "previous ReplicaSet delete failure" && obj.GetName() == previous.Name) {
						return cause
					}
					return c.Delete(ctx, obj, opts...)
				},
			}
			objects := []client.Object{current, stale}
			if operation != previousAbsentCase {
				objects = append(objects, previous)
			}
			r := newScaleReconcilerWith(t, funcs, objects...)
			r.Recorder = events.NewFakeRecorder(10)
			err := r.reconcileScale(t.Context(), d)
			if operation != previousAbsentCase {
				if !errors.Is(err, cause) {
					t.Fatalf("scale = %v, want %v", err, cause)
				}
				if d.Status.PreviousReplicaSet == nil || d.Status.CurrentReplicaSet.Name != current.Name {
					t.Fatalf("failed scale lost rollout references: %+v", d.Status)
				}
				fail = false
				if err := r.Get(t.Context(), client.ObjectKeyFromObject(current), &runtimev1alpha1.WorkloadReplicaSet{}); err != nil {
					t.Fatalf("current ReplicaSet lost: %v", err)
				}
				err = r.reconcileScale(t.Context(), d)
			}
			if !errors.Is(err, condition.ErrSkipReconciliation()) || d.Status.PreviousReplicaSet != nil {
				t.Fatalf("retry = %v, status = %+v", err, d.Status)
			}
			if err := r.reconcileScale(t.Context(), d); err != nil {
				t.Fatalf("repeat scale: %v", err)
			}
			for _, obsolete := range []*runtimev1alpha1.WorkloadReplicaSet{previous, stale} {
				if err := r.Get(t.Context(), client.ObjectKeyFromObject(obsolete), &runtimev1alpha1.WorkloadReplicaSet{}); !apierrors.IsNotFound(err) {
					t.Fatalf("obsolete ReplicaSet %s: %v", obsolete.Name, err)
				}
			}
			got := &runtimev1alpha1.WorkloadReplicaSet{}
			if err := r.Get(t.Context(), client.ObjectKeyFromObject(current), got); err != nil || *got.Spec.Replicas != *d.Spec.Replicas {
				t.Fatalf("current replicas after retry = %+v, %v", got.Spec.Replicas, err)
			}
			recorded := drainEvents(r.Recorder.(*events.FakeRecorder))
			wantEvents := 0
			if operation == updateDeniedCase {
				wantEvents = 1
			}
			if len(recorded) != wantEvents {
				t.Fatalf("refusal events = %v", recorded)
			}
		})
	}
}

func TestDeploymentTransientCreateFailureRetries(t *testing.T) {
	d := readyDeployment("")
	d.Status.Conditions = nil
	d.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadDeploymentConditionArtifact))
	cause := errors.New("API unavailable")
	fail := true
	r := newScaleReconcilerWith(t, interceptor.Funcs{
		Create: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.CreateOption) error {
			if fail {
				return cause
			}
			return c.Create(ctx, obj, opts...)
		},
	})
	before := d.DeepCopy()
	if err := r.reconcileDeploy(t.Context(), d); !errors.Is(err, cause) || !reflect.DeepEqual(d, before) {
		t.Fatalf("failed deploy = %v, status = %+v", err, d.Status)
	}
	fail = false
	if err := r.reconcileDeploy(t.Context(), d); !errors.Is(err, condition.ErrSkipReconciliation()) {
		t.Fatal(err)
	}
	d.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadDeploymentConditionSync))
	if err := r.reconcileDeploy(t.Context(), d); err != nil {
		t.Fatal(err)
	}
	list := &runtimev1alpha1.WorkloadReplicaSetList{}
	if err := r.List(t.Context(), list); err != nil || len(list.Items) != 1 {
		t.Fatalf("ReplicaSets = %d, %v", len(list.Items), err)
	}
	if !metav1.IsControlledBy(&list.Items[0], d) || list.Items[0].Name != d.Status.CurrentReplicaSet.Name {
		t.Fatalf("created ReplicaSet does not match rollout: %+v", list.Items[0])
	}
	if d.Status.PreviousReplicaSet == nil || d.Status.PreviousReplicaSet.Name != before.Status.CurrentReplicaSet.Name {
		t.Fatalf("rolling update previous = %+v, want %s", d.Status.PreviousReplicaSet, before.Status.CurrentReplicaSet.Name)
	}
}

func TestDeploymentReadyTracksPrerequisites(t *testing.T) {
	d := readyDeployment("")
	r := &WorkloadDeploymentReconciler{}
	if err := r.reconcileReady(t.Context(), d); !condition.IsStatusUnknown(err) {
		t.Fatalf("incomplete rollout = %v, want Unknown", err)
	}
	d.Status.SetConditions(
		condition.ReadyCondition(runtimev1alpha1.WorkloadDeploymentConditionArtifact),
		condition.ReadyCondition(runtimev1alpha1.WorkloadDeploymentConditionScale),
	)
	if err := r.reconcileReady(t.Context(), d); err != nil {
		t.Fatalf("completed rollout = %v", err)
	}
}
