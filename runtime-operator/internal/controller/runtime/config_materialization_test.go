package runtime

import (
	"encoding/base64"
	"reflect"
	"strings"
	"testing"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	configOverrideKey   = "override"
	configFixtureValue  = "value"
	inlineConfigKey     = "inline-only"
	localResourcesLabel = "component"
)

func TestMaterializeConfigLayerPrecedenceAndIsolation(t *testing.T) {
	const (
		configOrderKey = "config-order"
		firstLayer     = "first"
		lastLayer      = "last"
	)
	c := fake.NewClientBuilder().WithScheme(newScaleTestScheme(t)).WithObjects(
		&corev1.ConfigMap{ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: firstLayer}, Data: map[string]string{configOverrideKey: firstLayer, configOrderKey: firstLayer, "config-only": configFixtureValue}},
		&corev1.ConfigMap{ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: lastLayer}, Data: map[string]string{configOverrideKey: lastLayer, configOrderKey: lastLayer}},
		&corev1.Secret{ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: firstLayer}, Data: map[string][]byte{configOverrideKey: []byte("first-secret")}},
		&corev1.Secret{ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: lastLayer}, Data: map[string][]byte{configOverrideKey: []byte("last-secret"), "secret-only": []byte("secret")}},
	).Build()
	layer := &runtimev1alpha1.ConfigLayer{
		Config:     map[string]string{configOverrideKey: "inline", configOrderKey: "inline", inlineConfigKey: configFixtureValue},
		ConfigFrom: []corev1.LocalObjectReference{{Name: firstLayer}, {Name: lastLayer}},
		SecretFrom: []corev1.LocalObjectReference{{Name: firstLayer}, {Name: lastLayer}},
	}
	got, err := MaterializeConfigLayer(t.Context(), c, testNamespace, layer)
	if err != nil {
		t.Fatal(err)
	}
	want := map[string]string{configOverrideKey: "last-secret", configOrderKey: lastLayer, inlineConfigKey: configFixtureValue, "config-only": configFixtureValue, "secret-only": "secret"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("materialized config = %v, want %v", got, want)
	}
	got[inlineConfigKey] = "changed"
	if layer.Config[inlineConfigKey] != configFixtureValue {
		t.Fatal("materialization mutated inline config")
	}
	if _, err := MaterializeConfigLayer(t.Context(), c, "other-tenant", layer); !apierrors.IsNotFound(err) || !strings.Contains(err.Error(), "resolving local resources config") {
		t.Fatalf("cross-namespace config lookup: %v", err)
	}
	if _, err := MaterializeConfigLayer(t.Context(), c, testNamespace, &runtimev1alpha1.ConfigLayer{SecretFrom: []corev1.LocalObjectReference{{Name: "missing"}}}); !apierrors.IsNotFound(err) || !strings.Contains(err.Error(), "resolving local resources secret") {
		t.Fatalf("missing secret: %v", err)
	}
	if got, err := MaterializeConfigLayer(t.Context(), c, testNamespace, nil); err != nil || len(got) != 0 {
		t.Fatalf("absent config = %v, %v", got, err)
	}
}

// A pull secret yields its own auths for the image's registry and nothing
// else: no credential helper it names is run, and none of the operator's own
// registry credentials are handed out in its place.
func TestMaterializeImagePullSecret(t *testing.T) {
	credentials := base64.StdEncoding.EncodeToString([]byte("test-user:test-password"))
	operatorCredentials := base64.StdEncoding.EncodeToString([]byte("operator-user:operator-password"))
	// The operator's own credentials, which the unmatched registry must not get.
	t.Setenv("DOCKER_AUTH_CONFIG", `{"auths":{"another.example":{"auth":"`+operatorCredentials+`"}}}`)
	const privateImage = "registry.example/component:v1"
	// The store and helper cases name docker-credential-wasmcloud-absent, which
	// does not exist, so running it fails the case.
	const privateAuths = `"auths":{"registry.example":{"username":"test-user","password":"test-password"}}`
	for _, tt := range []struct {
		name, image, config string
		secretType          corev1.SecretType
		wantError           string
		wantCredentials     bool
	}{
		{name: "docker hub", image: "ubuntu:latest", config: `{"auths":{"https://index.docker.io/v1/":{"auth":"` + credentials + `"}}}`, secretType: corev1.SecretTypeDockerConfigJson, wantCredentials: true},
		{name: "private registry", image: privateImage, config: `{` + privateAuths + `}`, secretType: corev1.SecretTypeDockerConfigJson, wantCredentials: true},
		{name: "credential store", image: privateImage, config: `{"credsStore":"wasmcloud-absent",` + privateAuths + `}`, secretType: corev1.SecretTypeDockerConfigJson, wantCredentials: true},
		{name: "credential helper", image: privateImage, config: `{"credHelpers":{"registry.example":"wasmcloud-absent"},` + privateAuths + `}`, secretType: corev1.SecretTypeDockerConfigJson, wantCredentials: true},
		{name: "unmatched registry", image: "another.example/component:v1", config: `{"auths":{}}`, secretType: corev1.SecretTypeDockerConfigJson},
		{name: "wrong type", image: "ubuntu", config: `{}`, secretType: corev1.SecretTypeOpaque, wantError: "is not of type"},
		{name: "invalid JSON", image: "ubuntu", config: `{`, secretType: corev1.SecretTypeDockerConfigJson, wantError: "loading docker config"},
		{name: "invalid image", image: "invalid image", config: `{}`, secretType: corev1.SecretTypeDockerConfigJson, wantError: "parsing image reference"},
	} {
		t.Run(tt.name, func(t *testing.T) {
			secret := &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: "registry"}, Type: tt.secretType, Data: map[string][]byte{corev1.DockerConfigJsonKey: []byte(tt.config)}}
			c := fake.NewClientBuilder().WithScheme(newScaleTestScheme(t)).WithObjects(secret).Build()
			got, err := MaterializeImagePullSecret(t.Context(), c, testNamespace, secret.Name, tt.image)
			if tt.wantError != "" {
				if err == nil || !strings.Contains(err.Error(), tt.wantError) || got != nil {
					t.Fatalf("invalid pull secret: result=%v error=%v", got, err)
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			if tt.wantCredentials {
				if got.GetUsername() != "test-user" || got.GetPassword() != "test-password" {
					t.Fatalf("credentials = %v", got)
				}
			} else if got.GetUsername() != "" || got.GetPassword() != "" {
				t.Fatalf("unmatched registry received credentials: %v", got)
			}
			if _, err := MaterializeImagePullSecret(t.Context(), c, "other-tenant", secret.Name, tt.image); !apierrors.IsNotFound(err) {
				t.Fatalf("cross-namespace pull secret lookup: %v", err)
			}
		})
	}
}

func TestMaterializeLocalResources(t *testing.T) {
	c := fake.NewClientBuilder().WithScheme(newScaleTestScheme(t)).Build()
	spec := &runtimev1alpha1.LocalResources{
		Config: map[string]string{"setting": configFixtureValue}, Environment: &runtimev1alpha1.ConfigLayer{Config: map[string]string{"KEY": configFixtureValue}},
		AllowedHosts: []string{"https://api.example"}, AllowedIPNameLookups: []string{"api.example"}, AllowedHostLoopbackPorts: []string{"5432/tcp"},
		VolumeMounts: []runtimev1alpha1.VolumeMount{{Name: "data", MountPath: "/data", ReadOnly: true}},
	}
	got, err := materializeLocalResources(t.Context(), c, testNamespace, spec, localResourcesLabel)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got.Config, spec.Config) || !reflect.DeepEqual(got.Environment, spec.Environment.Config) || !reflect.DeepEqual(got.AllowedHosts, spec.AllowedHosts) || !reflect.DeepEqual(got.AllowedIpNameLookups, spec.AllowedIPNameLookups) || !reflect.DeepEqual(got.AllowedHostLoopbackPorts, spec.AllowedHostLoopbackPorts) {
		t.Fatalf("local resources were not preserved: %v", got)
	}
	if len(got.VolumeMounts) != 1 || got.VolumeMounts[0].Name != "data" || got.VolumeMounts[0].MountPath != "/data" || !got.VolumeMounts[0].ReadOnly {
		t.Fatalf("volume mounts = %v", got.VolumeMounts)
	}
	if got, err := materializeLocalResources(t.Context(), c, testNamespace, nil, localResourcesLabel); err != nil || got == nil {
		t.Fatalf("absent local resources: %v, %v", got, err)
	}
	spec.Environment.ConfigFrom = []corev1.LocalObjectReference{{Name: "missing"}}
	if _, err := materializeLocalResources(t.Context(), c, testNamespace, spec, localResourcesLabel); !apierrors.IsNotFound(err) || !strings.Contains(err.Error(), "for component") {
		t.Fatalf("invalid component environment: %v", err)
	}
}
