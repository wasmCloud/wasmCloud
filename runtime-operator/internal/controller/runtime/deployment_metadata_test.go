package runtime

import (
	"errors"
	"maps"
	"reflect"
	"testing"

	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	metadataLabelKey      = "metadata.example/release"
	metadataAnnotationKey = "metadata.example/config"
	metadataBeforeValue   = "before-update"
	metadataAfterValue    = "after-update"
)

func TestDeploymentTemplateMetadataChangesRollOut(t *testing.T) {
	for _, tc := range []struct {
		name        string
		annotations bool
		before      map[string]string
		after       map[string]string
	}{
		{name: "add label", after: map[string]string{metadataLabelKey: metadataAfterValue}},
		{name: "change label", before: map[string]string{metadataLabelKey: metadataBeforeValue}, after: map[string]string{metadataLabelKey: metadataAfterValue}},
		{name: "remove label", before: map[string]string{metadataLabelKey: metadataBeforeValue}},
		{name: "add annotation", annotations: true, after: map[string]string{metadataAnnotationKey: metadataAfterValue}},
		{name: "change annotation", annotations: true, before: map[string]string{metadataAnnotationKey: metadataBeforeValue}, after: map[string]string{metadataAnnotationKey: metadataAfterValue}},
		{name: "remove annotation", annotations: true, before: map[string]string{metadataAnnotationKey: metadataBeforeValue}},
	} {
		for _, policy := range []runtimev1alpha1.WorkloadDeployPolicy{
			runtimev1alpha1.WorkloadDeployPolicyRollingUpdate,
			runtimev1alpha1.WorkloadDeployPolicyRecreate,
		} {
			for _, mode := range []string{"metadata only", "rollback"} {
				t.Run(tc.name+"/"+string(policy)+"/"+mode, func(t *testing.T) {
					d := undeployedDeployment()
					d.Spec.DeployPolicy = policy
					d.Spec.Template.Spec.HostID = "metadata-host-a"
					if tc.annotations {
						d.Spec.Template.Annotations = maps.Clone(tc.before)
					} else {
						d.Spec.Template.Labels = maps.Clone(tc.before)
					}
					previous := ownedReplicaSetFor(t, d)
					before := previous.DeepCopy()
					objects := []client.Object{previous}
					current := previous
					if mode == "rollback" {
						other := d.DeepCopy()
						other.Spec.Template.Spec.HostID = "metadata-host-b"
						current = ownedReplicaSetFor(t, other)
						objects = append(objects, current)
						d.Status.PreviousReplicaSet = &runtimev1alpha1.WorkloadReplicaSetReference{Name: previous.Name}
					}
					d.Status.CurrentReplicaSet = &runtimev1alpha1.WorkloadReplicaSetReference{Name: current.Name}
					if tc.annotations {
						d.Spec.Template.Annotations = maps.Clone(tc.after)
					} else {
						d.Spec.Template.Labels = maps.Clone(tc.after)
					}
					stale := d.DeepCopy()
					r := newScaleReconciler(t, objects...)
					if err := r.reconcileSync(t.Context(), d); err == nil {
						t.Fatal("changed template metadata was reported synced")
					}
					for _, pass := range []*runtimev1alpha1.WorkloadDeployment{d, stale} {
						if err := r.reconcileDeploy(t.Context(), pass); !errors.Is(err, condition.ErrSkipReconciliation()) {
							t.Fatalf("deploy = %v", err)
						}
					}
					if d.Status.CurrentReplicaSet.Name == previous.Name || d.Status.CurrentReplicaSet.Name != stale.Status.CurrentReplicaSet.Name {
						t.Fatalf("metadata change did not select one fresh set: previous=%s current=%s retry=%s", previous.Name, d.Status.CurrentReplicaSet.Name, stale.Status.CurrentReplicaSet.Name)
					}
					if got := len(replicaSetNames(t, r)); got != len(objects)+1 {
						t.Fatalf("ReplicaSets = %d, want %d", got, len(objects)+1)
					}
					var target runtimev1alpha1.WorkloadReplicaSet
					if err := r.Get(t.Context(), client.ObjectKey{Namespace: d.Namespace, Name: d.Status.CurrentReplicaSet.Name}, &target); err != nil {
						t.Fatal(err)
					}
					if !reflect.DeepEqual(target.Spec.Template, d.Spec.Template) {
						t.Fatalf("rollout template = %+v, want %+v", target.Spec.Template, d.Spec.Template)
					}
					if err := r.reconcileSync(t.Context(), d); err != nil {
						t.Fatalf("new template not synced: %v", err)
					}
					if err := r.Get(t.Context(), client.ObjectKeyFromObject(previous), previous); err != nil {
						t.Fatal(err)
					}
					if !reflect.DeepEqual(previous.Spec, before.Spec) {
						t.Fatal("rollout changed the previous ReplicaSet's template")
					}

					// Exercise the next controller too: the new Workload must receive
					// the desired metadata, including removal of the old keys.
					rs := newCleanupReconciler(t, interceptor.Funcs{})
					if err := rs.reconcileScaleUp(t.Context(), &target); err != nil {
						t.Fatal(err)
					}
					workloads := &runtimev1alpha1.WorkloadList{}
					if err := rs.List(t.Context(), workloads); err != nil || len(workloads.Items) != 1 {
						t.Fatalf("created Workloads = %d, error = %v", len(workloads.Items), err)
					}
					metadata := workloads.Items[0].Labels
					key := metadataLabelKey
					if tc.annotations {
						metadata = workloads.Items[0].Annotations
						key = metadataAnnotationKey
					}
					if got, present := metadata[key]; got != tc.after[key] || present != (len(tc.after) != 0) {
						t.Fatalf("Workload metadata = %v, want %v", metadata, tc.after)
					}
				})
			}
		}
	}
}

func TestDeploymentMatchingLegacyReplicaSetStaysSynced(t *testing.T) {
	for _, metadata := range []bool{false, true} {
		d := readyDeployment("")
		d.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadDeploymentConditionArtifact))
		if metadata {
			d.Spec.Template.Labels = map[string]string{"legacy.example/team": "legacy-platform"}
			d.Spec.Template.Annotations = map[string]string{metadataAnnotationKey: "existing"}
		}
		// Its pre-upgrade name is deliberately unrelated to the current naming recipe.
		legacy := baseReplicaSet(testReplicaSetName)
		legacy.Spec.Template = *d.Spec.Template.DeepCopy()
		if !metadata {
			// API round trips can omit empty maps without changing the template.
			d.Spec.Template.Labels = map[string]string{}
			d.Spec.Template.Annotations = map[string]string{}
		}
		r := newScaleReconciler(t, legacy)
		if err := r.reconcileSync(t.Context(), d); err != nil {
			t.Fatalf("matching legacy template not synced: %v", err)
		}
		if err := r.reconcileDeploy(t.Context(), d); err != nil {
			t.Fatal(err)
		}
		if names := replicaSetNames(t, r); len(names) != 1 || names[0] != legacy.Name {
			t.Fatalf("matching legacy template rolled out: %v", names)
		}
	}
}
