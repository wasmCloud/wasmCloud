package runtime

import (
	"context"
	"errors"
	"reflect"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/testing/protocmp"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/events"
	"k8s.io/utils/ptr"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
)

const (
	testStableBilling   = "billing"
	testOwnedReplicaSet = "billing-set"
)

// The start request carries the workload's own namespace's ConfigMaps,
// Secrets and registry credentials, with Secrets overriding ConfigMaps and
// both overriding inline config, and leaves the Workload's spec alone.
func TestPlacementStartRequestUsesWorkloadNamespace(t *testing.T) {
	for _, namespace := range []string{testTenant, otherTenant} {
		t.Run(namespace, func(t *testing.T) {
			r, workload, bus := placementFixture(t)
			workload.Namespace = namespace
			before := workload.Spec.DeepCopy()
			if err := r.reconcilePlacement(t.Context(), workload); !errors.Is(err, condition.ErrSkipReconciliation()) {
				t.Fatal(err)
			}
			if workload.Status.WorkloadID != fixtureRuntimeID {
				t.Fatalf("recorded workload id = %q", workload.Status.WorkloadID)
			}
			if len(bus.requests) != 1 || bus.requests[0].Subject != startSubject {
				t.Fatalf("sent %d requests, last to %q; want one start", len(bus.requests), bus.gotSubject)
			}
			var got runtimev2.WorkloadStartRequest
			if err := protojson.Unmarshal(bus.requests[0].Data, &got); err != nil {
				t.Fatal(err)
			}
			httpConfig := placementEnvironment(httpLayer, namespace)
			httpConfig[hostAliasesKey] = "api." + namespace + ",api." + namespace + ".svc"
			want := &runtimev2.WorkloadStartRequest{WorkloadId: testWorkloadUID, Workload: &runtimev2.Workload{
				Namespace: namespace, Name: workload.Name, Annotations: map[string]string{"team": "placement-team"},
				WitWorld: &runtimev2.WitWorld{
					HostInterfaces: []*runtimev2.WitInterface{{
						Name: "inbound", Namespace: wasiNamespace, Package: httpLayer, Version: "0.2.6",
						Interfaces: []string{incomingHandler}, Config: httpConfig,
					}},
					Components: []*runtimev2.Component{
						{
							Name: fixtureComponent, Image: "registry.example/app:v1",
							ImagePullSecret: &runtimev2.ImagePullSecret{Username: "component-" + namespace, Password: "component-password"},
							ImagePullPolicy: runtimev2.ImagePullPolicy_IMAGE_PULL_POLICY_NEVER,
							LocalResources: &runtimev2.LocalResources{
								Environment: placementEnvironment(componentLayer, namespace),
								Config:      map[string]string{"component-setting": "on"},
								VolumeMounts: []*runtimev2.VolumeMount{
									{Name: fixtureCache, MountPath: "/cache"},
									{Name: fixtureData, MountPath: "/placement-data", ReadOnly: true},
								},
								AllowedHosts:             []string{"https://api.placement.example"},
								AllowedIpNameLookups:     []string{"api.placement.example"},
								AllowedHostLoopbackPorts: []string{"15432/tcp"},
							},
						},
						{
							Name: "helper", Image: "registry.example/helper:v1",
							ImagePullPolicy: runtimev2.ImagePullPolicy_IMAGE_PULL_POLICY_IF_NOT_PRESENT,
							LocalResources:  &runtimev2.LocalResources{},
						},
					},
				},
				Service: &runtimev2.Service{
					Image:           "sidecar.example/server:v2",
					ImagePullSecret: &runtimev2.ImagePullSecret{Username: "service-" + namespace, Password: "service-password"},
					ImagePullPolicy: runtimev2.ImagePullPolicy_IMAGE_PULL_POLICY_ALWAYS, MaxRestarts: 7,
					LocalResources: &runtimev2.LocalResources{
						Environment:  placementEnvironment(serviceLayer, namespace),
						Config:       map[string]string{"service-setting": "on"},
						VolumeMounts: []*runtimev2.VolumeMount{{Name: fixtureData, MountPath: "/srv"}},
					},
				},
				Volumes: []*runtimev2.Volume{
					{Name: fixtureCache, VolumeType: &runtimev2.Volume_EmptyDir{}},
					{Name: fixtureData, VolumeType: &runtimev2.Volume_HostPath{HostPath: &runtimev2.HostPathVolume{LocalPath: "/var/data"}}},
				},
				Owner: &runtimev2.WorkloadOwner{Kind: workloadKind, Name: workload.Name, Uid: testWorkloadUID},
			}}
			if diff := cmp.Diff(want, &got, protocmp.Transform()); diff != "" {
				t.Fatalf("start request (-want +got):\n%s", diff)
			}
			if !reflect.DeepEqual(before, &workload.Spec) {
				t.Fatal("placement mutated the workload spec")
			}
		})
	}
}

// The Service DNS names join the aliases a wasi:http incoming handler, p2 or
// p3, already lists, and every other interface is left alone.
func TestServiceDNSAliasesOnlyAffectIncomingHTTP(t *testing.T) {
	serviceAliases := "api." + testTenant + ",api." + testTenant + ".svc"
	incoming := []string{incomingHandler}
	for _, tc := range []struct {
		name, namespace, pkg string
		ifaces               []string
		config               map[string]string
		// want is the config after injection; nil means the interface is left alone.
		want map[string]string
	}{
		{name: "new config", namespace: wasiNamespace, pkg: httpLayer, ifaces: incoming,
			want: map[string]string{hostAliasesKey: serviceAliases}},
		{name: "existing aliases", namespace: wasiNamespace, pkg: httpLayer, ifaces: incoming,
			config: map[string]string{"keep": "value", hostAliasesKey: "gateway.example.com"},
			want:   map[string]string{"keep": "value", hostAliasesKey: "gateway.example.com," + serviceAliases}},
		{name: "aliases already listed", namespace: wasiNamespace, pkg: httpLayer, ifaces: incoming,
			config: map[string]string{hostAliasesKey: " api." + testTenant + ".svc, ,gateway.example.com,gateway.example.com"},
			want:   map[string]string{hostAliasesKey: "api." + testTenant + ".svc,gateway.example.com,api." + testTenant}},
		{name: "incoming among other interfaces", namespace: wasiNamespace, pkg: httpLayer,
			ifaces: []string{"outgoing-handler", incomingHandler},
			want:   map[string]string{hostAliasesKey: serviceAliases}},
		{name: "p3 handler", namespace: wasiNamespace, pkg: httpLayer, ifaces: []string{"handler"},
			config: map[string]string{hostAliasesKey: "gateway.example.com"},
			want:   map[string]string{hostAliasesKey: "gateway.example.com," + serviceAliases}},
		{name: "p3 client", namespace: wasiNamespace, pkg: httpLayer, ifaces: []string{"client"}},
		{name: "foreign namespace", namespace: "custom", pkg: httpLayer, ifaces: incoming},
		{name: "other package", namespace: wasiNamespace, pkg: "unrelated-package", ifaces: incoming},
		{name: "outgoing interface", namespace: wasiNamespace, pkg: httpLayer, ifaces: []string{"outgoing-handler"},
			config: map[string]string{hostAliasesKey: "outbound"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			interfaces := []*runtimev2.WitInterface{{Namespace: tc.namespace, Package: tc.pkg, Interfaces: tc.ifaces, Config: tc.config}}
			before := proto.Clone(interfaces[0]).(*runtimev2.WitInterface)
			injectServiceDNSAliases(interfaces, "api", testTenant)
			if tc.want == nil {
				if !proto.Equal(before, interfaces[0]) {
					t.Fatalf("unrelated interface changed: %v", interfaces[0])
				}
				return
			}
			if !reflect.DeepEqual(interfaces[0].Config, tc.want) {
				t.Fatalf("config = %v, want %v", interfaces[0].Config, tc.want)
			}
		})
	}
}

// A ConfigMap, Secret or registry credential placement cannot read fails it
// before any start is sent, naming what it was reading for.
func TestPlacementDependencyFailuresDoNotStart(t *testing.T) {
	for _, tc := range missingDependencies() {
		t.Run(tc.name, func(t *testing.T) {
			checkMissingDependency(t, tc, placementFixture, (*WorkloadReconciler).reconcilePlacement)
		})
	}
}

// A Kubernetes API failure while building the start request sends nothing and
// leaves the Workload as it was, so the next pass retries from the same state.
func TestPlacementAPIFailuresPreserveRetryState(t *testing.T) {
	errAPI := errors.New("API unavailable")
	for _, tc := range []struct {
		name  string
		setup func(*runtimev1alpha1.Workload)
		funcs interceptor.Funcs
	}{
		{
			name: "host lookup",
			funcs: interceptor.Funcs{List: func(ctx context.Context, c client.WithWatch, list client.ObjectList, opts ...client.ListOption) error {
				if _, ok := list.(*runtimev1alpha1.HostList); ok {
					return errAPI
				}
				return c.List(ctx, list, opts...)
			}},
		},
		{
			name: "owner lookup",
			setup: func(w *runtimev1alpha1.Workload) {
				owner := &runtimev1alpha1.WorkloadReplicaSet{ObjectMeta: metav1.ObjectMeta{Name: "owner", UID: "owner-uid"}}
				w.OwnerReferences = []metav1.OwnerReference{*metav1.NewControllerRef(owner, runtimev1alpha1.GroupVersion.WithKind("WorkloadReplicaSet"))}
			},
			funcs: interceptor.Funcs{Get: func(ctx context.Context, c client.WithWatch, key client.ObjectKey, obj client.Object, opts ...client.GetOption) error {
				if _, ok := obj.(*runtimev1alpha1.WorkloadReplicaSet); ok {
					return errAPI
				}
				return c.Get(ctx, key, obj, opts...)
			}},
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			r, workload, bus := placementFixture(t)
			if tc.setup != nil {
				tc.setup(workload)
			}
			r.Client = interceptor.NewClient(r.Client.(client.WithWatch), tc.funcs)
			before := workload.DeepCopy()
			if err := r.reconcilePlacement(t.Context(), workload); !errors.Is(err, errAPI) {
				t.Fatalf("placement error = %v", err)
			}
			if len(bus.requests) != 0 || !reflect.DeepEqual(workload, before) {
				t.Fatal("API failure started or changed the workload")
			}
		})
	}
}

// ownedFixture makes workload a replica of a WorkloadReplicaSet that a
// WorkloadDeployment controls and names current, as the operator builds them,
// and stores both. It returns them for a test to alter.
func ownedFixture(t *testing.T, r *WorkloadReconciler, workload *runtimev1alpha1.Workload) (*runtimev1alpha1.WorkloadDeployment, *runtimev1alpha1.WorkloadReplicaSet) {
	t.Helper()
	return ownedFixtureWithUID(t, r, workload, "deployment-uid")
}

func ownedFixtureWithUID(t *testing.T, r *WorkloadReconciler, workload *runtimev1alpha1.Workload, uid types.UID) (*runtimev1alpha1.WorkloadDeployment, *runtimev1alpha1.WorkloadReplicaSet) {
	t.Helper()
	controls := func(owner client.Object, kind string) []metav1.OwnerReference {
		return []metav1.OwnerReference{{
			APIVersion: runtimev1alpha1.GroupVersion.String(), Kind: kind,
			Name: owner.GetName(), UID: owner.GetUID(), Controller: ptr.To(true),
		}}
	}
	deployment := &runtimev1alpha1.WorkloadDeployment{
		ObjectMeta: metav1.ObjectMeta{Name: testStableBilling, Namespace: workload.Namespace, UID: uid},
	}
	replicaSet := &runtimev1alpha1.WorkloadReplicaSet{
		ObjectMeta: metav1.ObjectMeta{
			Name: testOwnedReplicaSet, Namespace: workload.Namespace, UID: "replicaset-uid",
			OwnerReferences: controls(deployment, workloadDeploymentKind),
		},
		Spec: runtimev1alpha1.WorkloadReplicaSetSpec{Template: runtimev1alpha1.WorkloadReplicaTemplate{Spec: *workload.Spec.DeepCopy()}},
	}
	deployment.Status.CurrentReplicaSet = &corev1.LocalObjectReference{Name: replicaSet.Name}
	workload.OwnerReferences = controls(replicaSet, workloadReplicaSetKind)
	workload.Annotations[workloadReplicaSetGenerationAnnotation] = replicaSet.Spec.Template.Hash()
	for _, obj := range []client.Object{deployment, replicaSet} {
		if err := r.Create(t.Context(), obj); err != nil {
			t.Fatal(err)
		}
	}
	return deployment, replicaSet
}

// sentOwner is the owner the last start request named.
func sentOwner(t *testing.T, bus *mockBus) *runtimev2.WorkloadOwner {
	t.Helper()
	if len(bus.requests) == 0 {
		t.Fatal("no start request was sent")
	}
	var req runtimev2.WorkloadStartRequest
	if err := protojson.Unmarshal(bus.requests[len(bus.requests)-1].Data, &req); err != nil {
		t.Fatal(err)
	}
	return req.GetWorkload().GetOwner()
}

// A Workload's owner is its verified controller chain, by kind, name and UID.
// Names written on the Workload, or a controller that cannot be shown to have
// made it, never make it another object's.
func TestPlacementOwner(t *testing.T) {
	self := &runtimev2.WorkloadOwner{Kind: workloadKind, Name: "placement", Uid: testWorkloadUID}
	deployment := &runtimev2.WorkloadOwner{Kind: workloadDeploymentKind, Name: testStableBilling, Uid: "deployment-uid"}
	for _, tc := range []struct {
		name  string
		setup func(*testing.T, *WorkloadReconciler, *runtimev1alpha1.Workload)
		want  *runtimev2.WorkloadOwner
		// refused is the start's refusal, when it is held.
		refused string
	}{
		{name: "bare", want: self},
		{name: "claimed names", setup: func(_ *testing.T, _ *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			w.Annotations[workloadDeploymentNameAnnotation] = testStableBilling
			w.Annotations["app.kubernetes.io/name"] = testStableBilling
		}, want: self},
		{name: "deployment replica", setup: func(t *testing.T, r *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			ownedFixture(t, r, w)
		}, want: deployment},
		{name: "previous set", setup: func(t *testing.T, r *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			d, rs := ownedFixture(t, r, w)
			d.Status.CurrentReplicaSet = &corev1.LocalObjectReference{Name: "newer"}
			d.Status.PreviousReplicaSet = &corev1.LocalObjectReference{Name: rs.Name}
			if err := r.Update(t.Context(), d); err != nil {
				t.Fatal(err)
			}
		}, want: deployment},
		{name: "set the deployment does not name", setup: func(t *testing.T, r *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			d, _ := ownedFixture(t, r, w)
			d.Status.CurrentReplicaSet = &corev1.LocalObjectReference{Name: "forged"}
			if err := r.Update(t.Context(), d); err != nil {
				t.Fatal(err)
			}
		}, refused: "does not name WorkloadReplicaSet"},
		{name: "spec the set does not make", setup: func(t *testing.T, r *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			ownedFixture(t, r, w)
			w.Spec.Components[0].Image = "registry.example/other:v1"
		}, refused: "is not what WorkloadReplicaSet"},
		{name: "copied generation", setup: func(t *testing.T, r *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			ownedFixture(t, r, w)
			delete(w.Annotations, workloadReplicaSetGenerationAnnotation)
		}, refused: "is not what WorkloadReplicaSet"},
		{name: "replaced set", setup: func(t *testing.T, r *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			ownedFixture(t, r, w)
			w.OwnerReferences[0].UID = "stale-uid"
		}, refused: "gone or was replaced"},
		{name: "replaced deployment", setup: func(t *testing.T, r *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			_, rs := ownedFixture(t, r, w)
			rs.OwnerReferences[0].UID = "stale-uid"
			if err := r.Update(t.Context(), rs); err != nil {
				t.Fatal(err)
			}
		}, refused: "gone or was replaced"},
		{name: "bare set", setup: func(t *testing.T, r *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			_, rs := ownedFixture(t, r, w)
			rs.OwnerReferences = nil
			if err := r.Update(t.Context(), rs); err != nil {
				t.Fatal(err)
			}
		}, want: &runtimev2.WorkloadOwner{Kind: workloadReplicaSetKind, Name: testOwnedReplicaSet, Uid: "replicaset-uid"}},
		{name: "daemonset instance", setup: func(t *testing.T, r *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			daemonSetFixture(t, r, w)
		}, want: &runtimev2.WorkloadOwner{Kind: workloadDaemonSetKind, Name: "agents", Uid: "daemonset-uid"}},
		{name: "outdated daemonset instance", setup: func(t *testing.T, r *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			ds := daemonSetFixture(t, r, w)
			ds.Spec.Template.Spec.Components[0].Image = "registry.example/app:v2"
			if err := r.Update(t.Context(), ds); err != nil {
				t.Fatal(err)
			}
		}, refused: "is not what WorkloadDaemonSet"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			r, workload, bus := placementFixture(t)
			recorder := events.NewFakeRecorder(4)
			r.Recorder = recorder
			if tc.setup != nil {
				tc.setup(t, r, workload)
			}
			err := r.reconcilePlacement(t.Context(), workload)
			if tc.refused != "" {
				if err == nil || !strings.Contains(err.Error(), tc.refused) {
					t.Fatalf("placement returned %v, want a refusal containing %q", err, tc.refused)
				}
				if got := condition.ErrorConditionFor(runtimev1alpha1.WorkloadConditionPlacement, err); got.Reason != reasonOwnerUnverified {
					t.Fatalf("placement condition reason = %q", got.Reason)
				}
				if recorded := drainEvents(recorder); len(recorded) != 1 || !strings.Contains(recorded[0], reasonOwnerUnverified) {
					t.Fatalf("events = %q", recorded)
				}
				if len(bus.requests) != 0 || workload.Status.Owner != nil {
					t.Fatalf("an unverified owner was placed: %d requests, owner %v", len(bus.requests), workload.Status.Owner)
				}
				return
			}
			if !errors.Is(err, condition.ErrSkipReconciliation()) {
				t.Fatal(err)
			}
			if got := sentOwner(t, bus); !proto.Equal(got, tc.want) {
				t.Fatalf("sent owner %v, want %v", got, tc.want)
			}
			want := runtimev1alpha1.WorkloadOwner{Kind: tc.want.Kind, Name: tc.want.Name, UID: types.UID(tc.want.Uid)}
			if workload.Status.Owner == nil || *workload.Status.Owner != want {
				t.Fatalf("recorded owner %v, want %v", workload.Status.Owner, want)
			}
		})
	}
}

// daemonSetFixture makes workload an instance of a WorkloadDaemonSet's
// current template and stores the set.
func daemonSetFixture(t *testing.T, r *WorkloadReconciler, workload *runtimev1alpha1.Workload) *runtimev1alpha1.WorkloadDaemonSet {
	t.Helper()
	daemonSet := &runtimev1alpha1.WorkloadDaemonSet{
		ObjectMeta: metav1.ObjectMeta{Name: "agents", Namespace: workload.Namespace, UID: "daemonset-uid"},
	}
	daemonSet.Spec.Template.Spec = *workload.Spec.DeepCopy()
	workload.Spec.HostID = testHeartbeatHostID
	workload.OwnerReferences = []metav1.OwnerReference{{
		APIVersion: runtimev1alpha1.GroupVersion.String(), Kind: workloadDaemonSetKind,
		Name: daemonSet.Name, UID: daemonSet.UID, Controller: ptr.To(true),
	}}
	workload.Annotations[workloadDaemonSetGenerationAnnotation] = daemonSet.Spec.Template.Hash()
	if err := r.Create(t.Context(), daemonSet); err != nil {
		t.Fatal(err)
	}
	return daemonSet
}

// Placement records the owner it sends and never changes it: a later start
// for another owner is refused before it reaches a host.
func TestPlacementRecordsOwnerOnce(t *testing.T) {
	r, workload, bus := placementFixture(t)
	recorder := events.NewFakeRecorder(4)
	r.Recorder = recorder
	deployment, replicaSet := ownedFixture(t, r, workload)
	if err := r.reconcilePlacement(t.Context(), workload); !errors.Is(err, condition.ErrSkipReconciliation()) {
		t.Fatal(err)
	}
	placed := *workload.Status.Owner

	workload.Status.WorkloadID = ""
	workload.OwnerReferences = nil
	err := r.reconcilePlacement(t.Context(), workload)
	if err == nil || !strings.Contains(err.Error(), "first placed for WorkloadDeployment billing") {
		t.Fatalf("re-placing for a new owner returned %v", err)
	}
	if got := condition.ErrorConditionFor(runtimev1alpha1.WorkloadConditionPlacement, err); got.Reason != reasonOwnerChanged {
		t.Fatalf("placement condition reason = %q", got.Reason)
	}
	if recorded := drainEvents(recorder); len(recorded) != 1 || !strings.Contains(recorded[0], reasonOwnerChanged) ||
		!strings.Contains(recorded[0], "recreate the Workload") {
		t.Fatalf("events = %q", recorded)
	}
	if *workload.Status.Owner != placed || workload.Status.WorkloadID != "" || len(bus.requests) != 1 {
		t.Fatalf("refused re-placement recorded %+v after %d requests", workload.Status, len(bus.requests))
	}

	// A deployment recreated under the same name is a different owner.
	for _, obj := range []client.Object{deployment, replicaSet} {
		if err := r.Delete(t.Context(), obj); err != nil {
			t.Fatal(err)
		}
	}
	ownedFixtureWithUID(t, r, workload, "recreated-uid")
	if err := r.reconcilePlacement(t.Context(), workload); err == nil || !strings.Contains(err.Error(), "uid recreated-uid") {
		t.Fatalf("re-placing for a recreated deployment returned %v", err)
	}
}
