package runtime

import (
	"context"
	"errors"
	"reflect"
	"strings"
	"testing"
	"time"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/client-go/tools/events"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"

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

// undeployedDeployment is a Deployment whose template Sync no longer matches,
// so reconcileDeploy has a rollout to make.
func undeployedDeployment() *runtimev1alpha1.WorkloadDeployment {
	d := readyDeployment("")
	d.Status.Conditions = nil
	d.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadDeploymentConditionArtifact))
	return d
}

// ownedReplicaSetFor is the ReplicaSet d's next rollout would create.
func ownedReplicaSetFor(t *testing.T, d *runtimev1alpha1.WorkloadDeployment) *runtimev1alpha1.WorkloadReplicaSet {
	t.Helper()
	rs := &runtimev1alpha1.WorkloadReplicaSet{
		ObjectMeta: metav1.ObjectMeta{
			Namespace: d.Namespace,
			Name:      deploymentReplicaSetName(d, d.Spec.Template.Hash()),
		},
		Spec: d.Spec.WorkloadReplicaSetSpec,
	}
	if err := controllerutil.SetControllerReference(d, rs, newScaleTestScheme(t)); err != nil {
		t.Fatalf("set owner: %v", err)
	}
	return rs
}

func replicaSetNames(t *testing.T, r *WorkloadDeploymentReconciler) []string {
	t.Helper()
	list := &runtimev1alpha1.WorkloadReplicaSetList{}
	if err := r.List(t.Context(), list); err != nil {
		t.Fatalf("list ReplicaSets: %v", err)
	}
	names := make([]string, 0, len(list.Items))
	for i := range list.Items {
		names = append(names, list.Items[i].Name)
	}
	return names
}

func referencedName(ref *corev1.LocalObjectReference) string {
	if ref == nil {
		return ""
	}
	return ref.Name
}

// A pass that read status before the last pass's write reached the cache sees
// the same rollout still to make, and must land on the same ReplicaSet.
func TestDeploymentStaleReadCreatesOneReplicaSet(t *testing.T) {
	r := newScaleReconciler(t)
	d := undeployedDeployment()
	stale := d.DeepCopy()

	if err := r.reconcileDeploy(t.Context(), d); !errors.Is(err, condition.ErrSkipReconciliation()) {
		t.Fatalf("first pass = %v", err)
	}
	if err := r.reconcileDeploy(t.Context(), stale); !errors.Is(err, condition.ErrSkipReconciliation()) {
		t.Fatalf("stale pass = %v", err)
	}

	names := replicaSetNames(t, r)
	if len(names) != 1 {
		t.Fatalf("ReplicaSets = %v, want one", names)
	}
	if !reflect.DeepEqual(stale.Status, d.Status) {
		t.Fatalf("stale pass status = %+v, want %+v", stale.Status, d.Status)
	}
	if got := referencedName(stale.Status.CurrentReplicaSet); got != names[0] {
		t.Fatalf("current = %q, want %q", got, names[0])
	}
	if got := referencedName(stale.Status.PreviousReplicaSet); got != testReplicaSetName {
		t.Fatalf("previous = %q, want %q", got, testReplicaSetName)
	}
}

// Status can name a ReplicaSet the cache has yet to hear of, which fails Sync.
// reconcileScale deletes the previous ReplicaSet, so one that is already
// current must not move there.
func TestDeploymentRolloutToCurrentReplicaSetKeepsPrevious(t *testing.T) {
	for _, tc := range []struct {
		name   string
		exists bool
	}{
		{name: "current ReplicaSet exists", exists: true},
		{name: "current ReplicaSet was deleted"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			d := undeployedDeployment()
			name := ownedReplicaSetFor(t, d).Name
			d.Status.CurrentReplicaSet = &corev1.LocalObjectReference{Name: name}
			d.Status.PreviousReplicaSet = &corev1.LocalObjectReference{Name: testPrevSetName}
			var objs []client.Object
			if tc.exists {
				objs = append(objs, ownedReplicaSetFor(t, d))
			}
			r := newScaleReconciler(t, objs...)

			if err := r.reconcileDeploy(t.Context(), d); !errors.Is(err, condition.ErrSkipReconciliation()) {
				t.Fatalf("reconcileDeploy = %v", err)
			}
			if got := referencedName(d.Status.CurrentReplicaSet); got != name {
				t.Errorf("current = %q, want %q", got, name)
			}
			if got := referencedName(d.Status.PreviousReplicaSet); got != testPrevSetName {
				t.Errorf("previous = %q, want %q", got, testPrevSetName)
			}
			if names := replicaSetNames(t, r); len(names) != 1 || names[0] != name {
				t.Errorf("ReplicaSets = %v, want only %s", names, name)
			}
		})
	}
}

// Going back to the template of a ReplicaSet still around rolls out to it.
func TestDeploymentRollbackAdoptsExistingReplicaSet(t *testing.T) {
	for _, tc := range []struct {
		name         string
		policy       runtimev1alpha1.WorkloadDeployPolicy
		wantPrevious string
	}{
		{
			name:         "rolling update",
			policy:       runtimev1alpha1.WorkloadDeployPolicyRollingUpdate,
			wantPrevious: testReplicaSetName,
		},
		{
			name:   "recreate",
			policy: runtimev1alpha1.WorkloadDeployPolicyRecreate,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			d := undeployedDeployment()
			d.Spec.DeployPolicy = tc.policy
			previous := ownedReplicaSetFor(t, d)
			d.Status.PreviousReplicaSet = &corev1.LocalObjectReference{Name: previous.Name}
			r := newScaleReconciler(t, previous)

			if err := r.reconcileDeploy(t.Context(), d); !errors.Is(err, condition.ErrSkipReconciliation()) {
				t.Fatalf("reconcileDeploy = %v", err)
			}
			if got := referencedName(d.Status.CurrentReplicaSet); got != previous.Name {
				t.Errorf("current = %q, want %q", got, previous.Name)
			}
			if got := referencedName(d.Status.PreviousReplicaSet); got != tc.wantPrevious {
				t.Errorf("previous = %q, want %q", got, tc.wantPrevious)
			}
			if names := replicaSetNames(t, r); len(names) != 1 {
				t.Errorf("ReplicaSets = %v, want only %s", names, previous.Name)
			}
		})
	}
}

// A name that cannot be rolled out to leaves the rollout where it was.
func TestDeploymentNameTakenLeavesStatus(t *testing.T) {
	cause := errors.New("API unavailable")
	for _, tc := range []struct {
		name        string
		occupant    func(*runtimev1alpha1.WorkloadReplicaSet)
		get         func(context.Context, client.WithWatch, client.ObjectKey, client.Object, ...client.GetOption) error
		wantUnknown bool
		wantErr     error
		wantMessage string
	}{
		{
			name: "being deleted",
			occupant: func(rs *runtimev1alpha1.WorkloadReplicaSet) {
				rs.DeletionTimestamp = &metav1.Time{Time: time.Now()}
				rs.Finalizers = []string{"test/hold"}
			},
			wantUnknown: true,
			wantMessage: "is being deleted",
		},
		{
			name:        "not controlled by the deployment",
			occupant:    func(rs *runtimev1alpha1.WorkloadReplicaSet) { rs.OwnerReferences = nil },
			wantMessage: "not controlled by this deployment",
		},
		{
			name: "a different template",
			occupant: func(rs *runtimev1alpha1.WorkloadReplicaSet) {
				rs.Spec.Template.Spec.HostID = "elsewhere"
			},
			wantMessage: "different template",
		},
		{
			name: "not in the cache yet",
			get: func(context.Context, client.WithWatch, client.ObjectKey, client.Object, ...client.GetOption) error {
				return apierrors.NewNotFound(schema.GroupResource{}, "uncached")
			},
			wantUnknown: true,
			wantMessage: "has not reached the cache",
		},
		{
			name: "unreadable",
			get: func(context.Context, client.WithWatch, client.ObjectKey, client.Object, ...client.GetOption) error {
				return cause
			},
			wantErr: cause,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			d := undeployedDeployment()
			occupant := ownedReplicaSetFor(t, d)
			if tc.occupant != nil {
				tc.occupant(occupant)
			}
			r := newScaleReconcilerWith(t, interceptor.Funcs{Get: tc.get}, occupant)
			before := d.DeepCopy()

			err := r.reconcileDeploy(t.Context(), d)
			if err == nil || errors.Is(err, condition.ErrSkipReconciliation()) {
				t.Fatalf("reconcileDeploy = %v, want a failure", err)
			}
			if got := condition.IsStatusUnknown(err); got != tc.wantUnknown {
				t.Errorf("status unknown = %v for %v, want %v", got, err, tc.wantUnknown)
			}
			if tc.wantErr != nil && !errors.Is(err, tc.wantErr) {
				t.Errorf("error = %v, want %v", err, tc.wantErr)
			}
			if !strings.Contains(err.Error(), tc.wantMessage) {
				t.Errorf("error = %v, want it to mention %q", err, tc.wantMessage)
			}
			if !reflect.DeepEqual(d.Status, before.Status) {
				t.Errorf("status = %+v, want %+v", d.Status, before.Status)
			}
		})
	}
}

// The name follows the resolved template and the Deployment's identity alone.
func TestDeploymentReplicaSetName(t *testing.T) {
	d := undeployedDeployment()
	same := d.DeepCopy()
	same.Status.CurrentReplicaSet = nil
	recreated := d.DeepCopy()
	recreated.UID = "recreated-uid"

	name := deploymentReplicaSetName(d, "hash")
	if got := deploymentReplicaSetName(same, "hash"); got != name {
		t.Errorf("same template named %q, want %q", got, name)
	}
	if got := deploymentReplicaSetName(d, "other-hash"); got == name {
		t.Errorf("another template shares the name %q", got)
	}
	if got := deploymentReplicaSetName(recreated, "hash"); got == name {
		t.Errorf("a recreated Deployment shares the name %q", got)
	}
}
