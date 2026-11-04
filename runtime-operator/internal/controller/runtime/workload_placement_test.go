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
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
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

func TestServiceDNSAliasesOnlyAffectIncomingHTTP(t *testing.T) {
	serviceAliases := "api." + testTenant + ",api." + testTenant + ".svc"
	for _, tc := range []struct {
		name, namespace, pkg, iface string
		config                      map[string]string
		// want is the config after injection; nil means the interface is left alone.
		want map[string]string
	}{
		{name: "new config", namespace: wasiNamespace, pkg: httpLayer, iface: incomingHandler,
			want: map[string]string{hostAliasesKey: serviceAliases}},
		{name: "existing config", namespace: wasiNamespace, pkg: httpLayer, iface: incomingHandler,
			config: map[string]string{"keep": "value", hostAliasesKey: "old"},
			want:   map[string]string{"keep": "value", hostAliasesKey: serviceAliases}},
		{name: "foreign namespace", namespace: "custom", pkg: httpLayer, iface: incomingHandler},
		{name: "other package", namespace: wasiNamespace, pkg: "unrelated-package", iface: incomingHandler},
		{name: "outgoing interface", namespace: wasiNamespace, pkg: httpLayer, iface: "outgoing-handler",
			config: map[string]string{hostAliasesKey: "outbound"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			interfaces := []*runtimev2.WitInterface{{Namespace: tc.namespace, Package: tc.pkg, Interfaces: []string{tc.iface}, Config: tc.config}}
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
	for _, tc := range []struct {
		name        string
		missing     client.Object
		setup       func(*runtimev1alpha1.Workload)
		wantContext string
	}{
		{name: "host interface config", missing: layerConfigMap(httpLayer),
			wantContext: "materializing host interface config for wasi/http:"},
		{name: "host interface secret", missing: layerSecret(httpLayer),
			wantContext: "materializing host interface config for wasi/http:"},
		{name: "component config", missing: layerConfigMap(componentLayer),
			wantContext: `materializing local resources config for component "` + fixtureComponent + `":`},
		{name: "component secret", missing: layerSecret(componentLayer),
			wantContext: `materializing local resources config for component "` + fixtureComponent + `":`},
		{name: "service config", missing: layerConfigMap(serviceLayer),
			wantContext: "materializing local resources config for service:"},
		{name: "service secret", missing: layerSecret(serviceLayer),
			wantContext: "materializing local resources config for service:"},
		{name: "component credentials", missing: fixturePullSecretObject(),
			wantContext: `materializing image pull secret for component "` + fixtureComponent + `":`},
		{name: "service credentials", missing: fixturePullSecretObject(),
			setup:       func(w *runtimev1alpha1.Workload) { w.Spec.Components[0].ImagePullSecret = nil },
			wantContext: "materializing image pull secret for service:"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			r, workload, bus := placementFixture(t)
			if tc.setup != nil {
				tc.setup(workload)
			}
			if err := r.Delete(t.Context(), tc.missing); err != nil {
				t.Fatal(err)
			}
			before := workload.DeepCopy()
			err := r.reconcilePlacement(t.Context(), workload)
			if !apierrors.IsNotFound(err) || !strings.HasPrefix(err.Error(), tc.wantContext) {
				t.Fatalf("placement error = %v, want NotFound starting %q", err, tc.wantContext)
			}
			if len(bus.requests) != 0 {
				t.Fatalf("sent %d requests with invalid dependencies", len(bus.requests))
			}
			if !reflect.DeepEqual(workload, before) {
				t.Fatal("failed placement changed the workload")
			}
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
