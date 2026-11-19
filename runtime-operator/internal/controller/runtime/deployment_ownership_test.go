package runtime

import (
	"context"
	"errors"
	"testing"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// foreignReplicaSet has the current ReplicaSet's name but another controller,
// as after the Deployment was deleted and re-applied with --cascade=orphan.
func foreignReplicaSet() *runtimev1alpha1.WorkloadReplicaSet {
	rs := availableReplicaSet(testReplicaSetName)
	rs.OwnerReferences[0].UID = types.UID("someone-else")
	return rs
}

func countUpdates(updates *int) interceptor.Funcs {
	return interceptor.Funcs{
		Update: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.UpdateOption) error {
			*updates++
			return c.Update(ctx, obj, opts...)
		},
	}
}

// A Deployment never scales or reports through a ReplicaSet it does not
// control, even one with the name it recorded.
func TestDeploymentLeavesReplicaSetOfAnotherOwner(t *testing.T) {
	t.Run("sync", func(t *testing.T) {
		wd := readyDeployment("")
		wd.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadDeploymentConditionArtifact))
		r := newScaleReconciler(t, wd, foreignReplicaSet())
		if err := r.reconcileSync(context.Background(), wd); !condition.IsStatusUnknown(err) {
			t.Fatalf("reconcileSync returned %v, want an unknown condition", err)
		}
	})

	t.Run("scale", func(t *testing.T) {
		wd := readyDeployment("")
		wd.Spec.Replicas = new(testReplicas + 1)
		updates := 0
		r := newScaleReconcilerWith(t, countUpdates(&updates), wd, foreignReplicaSet())
		if err := r.reconcileScale(context.Background(), wd); !condition.IsStatusUnknown(err) {
			t.Fatalf("reconcileScale returned %v, want an unknown condition", err)
		}
		if updates != 0 {
			t.Errorf("scaled a ReplicaSet it does not control (%d updates)", updates)
		}
	})
}

// Scaling has side effects before the status patch runs, so it first confirms
// the Deployment it read is still the live one.
func TestDeploymentScaleConfirmsDeployment(t *testing.T) {
	for _, tc := range []struct {
		name       string
		live       func(*runtimev1alpha1.WorkloadDeployment) client.Reader
		wantUpdate bool
		wantErr    func(error) bool
	}{
		{
			name: "unchanged",
			live: func(wd *runtimev1alpha1.WorkloadDeployment) client.Reader {
				return fake.NewClientBuilder().WithScheme(newScaleTestScheme(t)).WithObjects(wd.DeepCopy()).Build()
			},
			wantUpdate: true,
			wantErr:    func(err error) bool { return err == nil },
		},
		{
			name: "changed since read",
			live: func(wd *runtimev1alpha1.WorkloadDeployment) client.Reader {
				newer := wd.DeepCopy()
				newer.ResourceVersion = "999"
				return fake.NewClientBuilder().WithScheme(newScaleTestScheme(t)).WithObjects(newer).Build()
			},
			wantErr: func(err error) bool { return errors.Is(err, condition.ErrNoop()) },
		},
		{
			name: "deleted",
			live: func(*runtimev1alpha1.WorkloadDeployment) client.Reader {
				return fake.NewClientBuilder().WithScheme(newScaleTestScheme(t)).Build()
			},
			wantErr: func(err error) bool { return errors.Is(err, condition.ErrNoop()) },
		},
		{
			name:    "read fails",
			live:    func(*runtimev1alpha1.WorkloadDeployment) client.Reader { return failingReader{} },
			wantErr: func(err error) bool { return err != nil && !errors.Is(err, condition.ErrNoop()) },
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			wd := readyDeployment("")
			wd.ResourceVersion = "7"
			wd.Spec.Replicas = new(testReplicas + 1)
			updates := 0
			r := newScaleReconcilerWith(t, countUpdates(&updates), availableReplicaSet(testReplicaSetName))
			r.APIReader = tc.live(wd)

			err := r.reconcileScale(context.Background(), wd)
			if !tc.wantErr(err) {
				t.Fatalf("reconcileScale returned %v", err)
			}
			if got := updates > 0; got != tc.wantUpdate {
				t.Errorf("scaled = %v, want %v", got, tc.wantUpdate)
			}
		})
	}
}

// Retiring the previous ReplicaSet reads it past the cache first; a read that
// fails leaves it in place and fails the pass.
func TestDeploymentRetirePreviousReadFailure(t *testing.T) {
	wd := readyDeployment(testPrevSetName)
	r := newScaleReconciler(t, wd, availableReplicaSet(testReplicaSetName), availableReplicaSet(testPrevSetName))
	r.APIReader = failingReader{}

	err := r.deletePreviousReplicaSet(context.Background(), wd, testPrevSetName)
	if err == nil {
		t.Fatal("retired the previous ReplicaSet without reading it")
	}
	if getErr := r.Get(context.Background(), client.ObjectKey{Namespace: testNamespace, Name: testPrevSetName},
		&runtimev1alpha1.WorkloadReplicaSet{}); apierrors.IsNotFound(getErr) {
		t.Fatal("the previous ReplicaSet was deleted anyway")
	}
}

// With no replica count set (the /scale subresource unset), a Deployment
// expects no replicas while still reporting what its ReplicaSets run.
func TestDeploymentScaleStatusWithoutReplicaCount(t *testing.T) {
	wd := readyDeployment("")
	wd.Spec.Replicas = nil
	r := newScaleReconciler(t)
	status := r.setScaleStatus(context.Background(), wd, []runtimev1alpha1.WorkloadReplicaSet{*availableReplicaSet(testReplicaSetName)})
	if status.Expected != 0 || status.Current != testReplicas || status.Ready != testReplicas {
		t.Fatalf("status = %+v", status)
	}
}
