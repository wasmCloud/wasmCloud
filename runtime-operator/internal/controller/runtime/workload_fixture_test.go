package runtime

import (
	"context"
	"fmt"
	"reflect"
	"strings"
	"testing"
	"time"

	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

// Names in the Workload placementFixture builds. Each config layer is named
// after the part of the Workload that carries it.
const (
	fixtureRuntimeID   = "runtime-workload-id"
	fixtureComponent   = "placement-app"
	fixturePullSecret  = "registry-credentials"
	fixtureOverrideKey = "OVERRIDE"
	fixtureCache       = "cache"
	fixtureData        = "data"
	componentLayer     = "component"
	serviceLayer       = "service"
	httpLayer          = "http"
	startSubject       = "runtime.host." + testHeartbeatHostID + ".workload.start"
	statusSubject      = "runtime.host." + testHeartbeatHostID + ".workload.status"
	wasiNamespace      = "wasi"
	incomingHandler    = "incoming-handler"
	hostAliasesKey     = "host-aliases"
)

func layerConfigName(layer string) string { return layer + "-config" }

func layerSecretName(layer string) string { return layer + "-secret" }

// layerConfigMap and layerSecret name the objects layer reads in testTenant,
// for a test to delete.
func layerConfigMap(layer string) client.Object {
	return &corev1.ConfigMap{ObjectMeta: metav1.ObjectMeta{Namespace: testTenant, Name: layerConfigName(layer)}}
}

func layerSecret(layer string) client.Object {
	return &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Namespace: testTenant, Name: layerSecretName(layer)}}
}

func fixturePullSecretObject() client.Object {
	return &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Namespace: testTenant, Name: fixturePullSecret}}
}

// placementLayer sets fixtureOverrideKey inline; the layer's ConfigMap and
// then its Secret override it.
func placementLayer(layer string) *runtimev1alpha1.ConfigLayer {
	return &runtimev1alpha1.ConfigLayer{
		Config:     map[string]string{"INLINE": layer, fixtureOverrideKey: "from-inline"},
		ConfigFrom: []corev1.LocalObjectReference{{Name: layerConfigName(layer)}},
		SecretFrom: []corev1.LocalObjectReference{{Name: layerSecretName(layer)}},
	}
}

// placementEnvironment is what placementLayer(layer) resolves to in namespace.
func placementEnvironment(layer, namespace string) map[string]string {
	return map[string]string{
		"INLINE":           layer,
		"CONFIG":           layer + "-" + namespace,
		"TOKEN":            layer + "-token-" + namespace,
		fixtureOverrideKey: "from-secret",
	}
}

// placementObjects are the ConfigMaps, Secrets and registry credentials the
// fixture Workload reads, with values that name namespace.
func placementObjects(namespace string) []client.Object {
	var objects []client.Object
	for _, layer := range []string{componentLayer, serviceLayer, httpLayer} {
		objects = append(objects,
			&corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{Namespace: namespace, Name: layerConfigName(layer)},
				Data:       map[string]string{"CONFIG": layer + "-" + namespace, fixtureOverrideKey: "from-config"},
			},
			&corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{Namespace: namespace, Name: layerSecretName(layer)},
				Data: map[string][]byte{
					"TOKEN":            []byte(layer + "-token-" + namespace),
					fixtureOverrideKey: []byte("from-secret"),
				},
			},
		)
	}
	credentials := fmt.Sprintf(`{"auths":{`+
		`"registry.example":{"username":"component-%[1]s","password":"component-password"},`+
		`"sidecar.example":{"username":"service-%[1]s","password":"service-password"}}}`, namespace)
	return append(objects, &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{Namespace: namespace, Name: fixturePullSecret},
		Type:       corev1.SecretTypeDockerConfigJson,
		Data:       map[string][]byte{corev1.DockerConfigJsonKey: []byte(credentials)},
	})
}

func rpcReply(t *testing.T, response proto.Message) *wasmbus.Message {
	t.Helper()
	data, err := protojson.Marshal(response)
	if err != nil {
		t.Fatal(err)
	}
	return &wasmbus.Message{Data: data}
}

// startReply is the host's answer to a start, placing workloadID in state.
func startReply(t *testing.T, workloadID string, state runtimev2.WorkloadState, message string) *wasmbus.Message {
	t.Helper()
	return rpcReply(t, &runtimev2.WorkloadStartResponse{WorkloadStatus: &runtimev2.WorkloadStatus{
		WorkloadId: workloadID, WorkloadState: state, Message: message,
	}})
}

// startedReply is the host's answer to a start it accepted.
func startedReply(t *testing.T) *wasmbus.Message {
	t.Helper()
	return startReply(t, fixtureRuntimeID, runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING, "")
}

// refusedReply is the host's answer to a start of workloadID it turned down.
func refusedReply(t *testing.T, workloadID, reason string) *wasmbus.Message {
	t.Helper()
	return startReply(t, workloadID, runtimev2.WorkloadState_WORKLOAD_STATE_ERROR, reason)
}

// statusReply is the host's answer to a status check of workloadID.
func statusReply(t *testing.T, workloadID string, state runtimev2.WorkloadState, message string) *wasmbus.Message {
	t.Helper()
	return rpcReply(t, &runtimev2.WorkloadStatusResponse{WorkloadStatus: &runtimev2.WorkloadStatus{
		WorkloadId: workloadID, WorkloadState: state, Message: message,
	}})
}

// recordPlacement records the error reconcilePlacement returned as the
// Workload's Placement condition, as the condition pipeline does.
func recordPlacement(workload *runtimev1alpha1.Workload, err error) {
	placement := condition.ErrorCondition(runtimev1alpha1.WorkloadConditionPlacement, "Reconcile", err)
	if condition.IsStatusUnknown(err) {
		placement = condition.UnknownCondition(runtimev1alpha1.WorkloadConditionPlacement, "Reconcile", err.Error())
	}
	workload.Status.SetConditions(placement)
}

// placementFixture is a Workload in testTenant whose components, service and
// host interface each read a config layer, with its host selected and those
// objects present in both testTenant and otherTenant. The bus accepts starts.
func placementFixture(t *testing.T) (*WorkloadReconciler, *runtimev1alpha1.Workload, *mockBus) {
	t.Helper()
	host := &runtimev1alpha1.Host{
		ObjectMeta:  metav1.ObjectMeta{Name: testHeartbeatHostID, Namespace: testNamespace},
		HostID:      testHeartbeatHostID,
		Environment: testTenant,
	}
	objects := append(placementObjects(testTenant), placementObjects(otherTenant)...)
	c := newWorkloadClient(t, append(objects, host)...)
	bus := &mockBus{reply: startedReply(t)}
	pullSecret := &corev1.LocalObjectReference{Name: fixturePullSecret}
	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{
			Name: "placement", Namespace: testTenant, UID: testWorkloadUID,
			Annotations: map[string]string{"team": "placement-team"},
		},
		Spec: runtimev1alpha1.WorkloadSpec{
			Components: []runtimev1alpha1.WorkloadComponent{
				{
					Name: fixtureComponent, Image: "registry.example/app:v1",
					ImagePullSecret: pullSecret, ImagePullPolicy: corev1.PullNever,
					LocalResources: &runtimev1alpha1.LocalResources{
						Environment: placementLayer(componentLayer),
						Config:      map[string]string{"component-setting": "on"},
						VolumeMounts: []runtimev1alpha1.VolumeMount{
							{Name: fixtureCache, MountPath: "/cache"},
							{Name: fixtureData, MountPath: "/placement-data", ReadOnly: true},
						},
						AllowedHosts:             []string{"https://api.placement.example"},
						AllowedIPNameLookups:     []string{"api.placement.example"},
						AllowedHostLoopbackPorts: []string{"15432/tcp"},
					},
				},
				{Name: "helper", Image: "registry.example/helper:v1", ImagePullPolicy: corev1.PullIfNotPresent},
			},
			Service: &runtimev1alpha1.WorkloadService{
				Image: "sidecar.example/server:v2", ImagePullSecret: pullSecret,
				ImagePullPolicy: corev1.PullAlways, MaxRestarts: 7,
				LocalResources: &runtimev1alpha1.LocalResources{
					Environment:  placementLayer(serviceLayer),
					Config:       map[string]string{"service-setting": "on"},
					VolumeMounts: []runtimev1alpha1.VolumeMount{{Name: fixtureData, MountPath: "/srv"}},
				},
			},
			Volumes: []runtimev1alpha1.Volume{
				{Name: fixtureCache, EphemeralVolume: &runtimev1alpha1.EphemeralVolume{}},
				{Name: fixtureData, HostPathVolume: &runtimev1alpha1.HostPathVolume{Path: "/var/data"}},
			},
			HostInterfaces: []runtimev1alpha1.HostInterface{{
				Name: "inbound", Namespace: wasiNamespace, Package: httpLayer, Version: "0.2.6",
				Interfaces: []string{incomingHandler}, ConfigLayer: *placementLayer(httpLayer),
			}},
			Kubernetes: &runtimev1alpha1.KubernetesSpec{Service: &runtimev1alpha1.KubernetesServiceRef{Name: "api"}},
		},
		Status: runtimev1alpha1.WorkloadStatus{HostID: testHeartbeatHostID, Environment: testTenant},
	}
	return &WorkloadReconciler{Client: c, Bus: bus, OperatorNamespace: testNamespace}, workload, bus
}

// unplacedFixture is placementFixture before host selection.
func unplacedFixture(t *testing.T) (*WorkloadReconciler, *runtimev1alpha1.Workload, *mockBus) {
	t.Helper()
	r, workload, bus := placementFixture(t)
	workload.Status = runtimev1alpha1.WorkloadStatus{}
	return r, workload, bus
}

// placedFixture is placementFixture after a start the host accepted.
func placedFixture(t *testing.T) (*WorkloadReconciler, *runtimev1alpha1.Workload, *mockBus) {
	t.Helper()
	r, workload, bus := placementFixture(t)
	workload.Status.WorkloadID = fixtureRuntimeID
	workload.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadConditionPlacement))
	return r, workload, bus
}

// expireStartWaits lets each start held back after a refusal go now.
func expireStartWaits(r *WorkloadReconciler) {
	r.startWaits.Range(func(uid, wait any) bool {
		expired := wait.(startWait)
		expired.until = time.Time{}
		r.startWaits.Store(uid, expired)
		return true
	})
}

// missingDependency is an object the fixture Workload reads, to delete, and
// the context of the NotFound error reading it then fails with.
type missingDependency struct {
	name        string
	missing     client.Object
	setup       func(*runtimev1alpha1.Workload)
	wantContext string
}

func missingDependencies() []missingDependency {
	componentContext := `materializing local resources config for component "` + fixtureComponent + `":`
	return []missingDependency{
		{name: "host interface config", missing: layerConfigMap(httpLayer),
			wantContext: "materializing host interface config for wasi/http:"},
		{name: "host interface secret", missing: layerSecret(httpLayer),
			wantContext: "materializing host interface config for wasi/http:"},
		{name: "component config", missing: layerConfigMap(componentLayer), wantContext: componentContext},
		{name: "component secret", missing: layerSecret(componentLayer), wantContext: componentContext},
		{name: "service config", missing: layerConfigMap(serviceLayer),
			wantContext: "materializing local resources config for service:"},
		{name: "service secret", missing: layerSecret(serviceLayer),
			wantContext: "materializing local resources config for service:"},
		{name: "component credentials", missing: fixturePullSecretObject(),
			wantContext: `materializing image pull secret for component "` + fixtureComponent + `":`},
		{name: "service credentials", missing: fixturePullSecretObject(),
			setup:       func(w *runtimev1alpha1.Workload) { w.Spec.Components[0].ImagePullSecret = nil },
			wantContext: "materializing image pull secret for service:"},
	}
}

// checkMissingDependency deletes tc.missing from fixture's objects, runs
// reconcile, and checks it failed reading it without sending anything or
// changing the Workload.
func checkMissingDependency(
	t *testing.T, tc missingDependency,
	fixture func(*testing.T) (*WorkloadReconciler, *runtimev1alpha1.Workload, *mockBus),
	reconcile func(*WorkloadReconciler, context.Context, *runtimev1alpha1.Workload) error,
) {
	t.Helper()
	r, workload, bus := fixture(t)
	if tc.setup != nil {
		tc.setup(workload)
	}
	if err := r.Delete(t.Context(), tc.missing); err != nil {
		t.Fatal(err)
	}
	before := workload.DeepCopy()
	err := reconcile(r, t.Context(), workload)
	if !apierrors.IsNotFound(err) || !strings.HasPrefix(err.Error(), tc.wantContext) {
		t.Fatalf("error = %v, want NotFound starting %q", err, tc.wantContext)
	}
	if len(bus.requests) != 0 {
		t.Fatalf("sent %d requests with invalid dependencies", len(bus.requests))
	}
	if !reflect.DeepEqual(workload, before) {
		t.Fatal("the failure changed the workload")
	}
}
