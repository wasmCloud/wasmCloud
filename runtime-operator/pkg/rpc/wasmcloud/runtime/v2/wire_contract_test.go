package v2

import (
	"bytes"
	_ "embed"
	"slices"
	"testing"

	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
)

const startWireFixture = `{
  "workloadId": "runtime-id",
  "workload": {
    "namespace": "tenant-a",
    "name": "echo",
    "annotations": {
      "team": "runtime"
    },
    "service": {
      "image": "registry.example/service:v1",
      "maxRestarts": "3",
      "imagePullPolicy": "IMAGE_PULL_POLICY_ALWAYS",
      "imagePullSecret": {
        "username": "service-user",
        "password": "service-password"
      },
      "localResources": {
        "environment": {
          "MODE": "service"
        }
      }
    },
    "witWorld": {
      "components": [
        {
          "name": "app",
          "image": "registry.example/app:v1",
          "poolSize": 4,
          "maxInvocations": -1,
          "maxConcurrency": 2,
          "reclaimWindowSeconds": 30,
          "reclaimMinInstances": 1,
          "imagePullPolicy": "IMAGE_PULL_POLICY_IF_NOT_PRESENT",
          "imagePullSecret": {
            "username": "app-user",
            "password": "app-password"
          },
          "localResources": {
            "memoryLimitMb": 64,
            "cpuLimit": 2,
            "config": {
              "cache": "on"
            },
            "environment": {
              "MODE": "app"
            },
            "volumeMounts": [
              {
                "name": "data",
                "mountPath": "/data",
                "readOnly": true
              }
            ],
            "allowedHosts": [
              "https://api.example:443"
            ],
            "allowedIpNameLookups": [
              "*.example"
            ],
            "allowedHostLoopbackPorts": [
              "5432/tcp"
            ]
          }
        }
      ],
      "hostInterfaces": [
        {
          "namespace": "wasi",
          "package": "http",
          "version": "0.2.0",
          "interfaces": [
            "incoming-handler"
          ],
          "name": "http",
          "config": {
            "port": "8080"
          }
        }
      ]
    },
    "volumes": [
      {
        "name": "data",
        "hostPath": {
          "localPath": "/srv/data"
        }
      },
      {
        "name": "scratch",
        "emptyDir": {}
      }
    ]
  }
}`

// The fixture pins the v2 field numbers and wire types independently of the encoder.
//
//go:embed testdata/workload-start-v2.bin
var startWireBinary []byte

func TestWorkloadStartWireContract(t *testing.T) {
	for _, codec := range []string{"json", "protobuf"} {
		t.Run(codec, func(t *testing.T) {
			request := &WorkloadStartRequest{}
			if err := protojson.Unmarshal([]byte(startWireFixture), request); err != nil {
				t.Fatal(err)
			}
			var data []byte
			var err error
			decoded := &WorkloadStartRequest{}
			if codec == "json" {
				data, err = protojson.Marshal(request)
				if err == nil {
					err = protojson.Unmarshal(data, decoded)
				}
			} else {
				if err = proto.Unmarshal(startWireBinary, decoded); err != nil {
					t.Fatal(err)
				}
				data, err = (proto.MarshalOptions{Deterministic: true}).Marshal(request)
				if err == nil && !bytes.Equal(data, startWireBinary) {
					t.Fatal("protobuf encoding differs from the v2 binary fixture")
				}
			}
			if err != nil || !proto.Equal(request, decoded) {
				t.Fatalf("round trip changed the start request: %v", err)
			}
			w := decoded.GetWorkload()
			if decoded.GetWorkloadId() != "runtime-id" ||
				w.GetNamespace() != "tenant-a" ||
				w.GetName() != "echo" ||
				w.GetAnnotations()["team"] != "runtime" {
				t.Fatal("workload identity or annotations changed")
			}
			assertStartComponent(t, w.GetWitWorld())
			assertStartVolumesAndService(t, w)
			assertStartHostInterfaces(t, w.GetWitWorld())
		})
	}
}

func assertStartComponent(t *testing.T, world *WitWorld) {
	t.Helper()
	components := world.GetComponents()
	if len(components) != 1 {
		t.Fatalf("components = %d", len(components))
	}
	c := components[0]
	if c.GetName() != "app" ||
		c.GetImage() != "registry.example/app:v1" ||
		c.GetPoolSize() != 4 ||
		c.GetMaxInvocations() != -1 ||
		c.GetMaxConcurrency() != 2 ||
		c.GetReclaimWindowSeconds() != 30 ||
		c.GetReclaimMinInstances() != 1 ||
		c.GetImagePullPolicy() != ImagePullPolicy_IMAGE_PULL_POLICY_IF_NOT_PRESENT {
		t.Fatalf("component image or signed instance limits changed: %v", c)
	}
	if c.GetImagePullSecret().GetUsername() != "app-user" ||
		c.GetImagePullSecret().GetPassword() != "app-password" {
		t.Fatal("component registry credentials changed")
	}
	resources := c.GetLocalResources()
	if resources.GetMemoryLimitMb() != 64 ||
		resources.GetCpuLimit() != 2 ||
		resources.GetConfig()["cache"] != "on" ||
		resources.GetEnvironment()["MODE"] != "app" {
		t.Fatal("component resources or configuration changed")
	}
	if !slices.Equal(resources.GetAllowedHosts(), []string{"https://api.example:443"}) ||
		!slices.Equal(resources.GetAllowedIpNameLookups(), []string{"*.example"}) ||
		!slices.Equal(resources.GetAllowedHostLoopbackPorts(), []string{"5432/tcp"}) {
		t.Fatal("egress policy changed")
	}
	mounts := resources.GetVolumeMounts()
	if len(mounts) != 1 ||
		mounts[0].GetName() != "data" ||
		mounts[0].GetMountPath() != "/data" ||
		!mounts[0].GetReadOnly() {
		t.Fatal("volume mount changed")
	}
}

func assertStartVolumesAndService(t *testing.T, w *Workload) {
	t.Helper()
	volumes := w.GetVolumes()
	if len(volumes) != 2 ||
		volumes[0].GetName() != "data" ||
		volumes[0].GetHostPath().GetLocalPath() != "/srv/data" ||
		volumes[0].GetEmptyDir() != nil ||
		volumes[1].GetName() != "scratch" ||
		volumes[1].GetHostPath() != nil ||
		volumes[1].GetEmptyDir() == nil {
		t.Fatal("volume oneof changed")
	}
	service := w.GetService()
	if service.GetImage() != "registry.example/service:v1" ||
		service.GetMaxRestarts() != 3 ||
		service.GetImagePullPolicy() != ImagePullPolicy_IMAGE_PULL_POLICY_ALWAYS ||
		service.GetImagePullSecret().GetUsername() != "service-user" ||
		service.GetImagePullSecret().GetPassword() != "service-password" ||
		service.GetLocalResources().GetEnvironment()["MODE"] != "service" {
		t.Fatal("service configuration changed")
	}
}

func assertStartHostInterfaces(t *testing.T, world *WitWorld) {
	t.Helper()
	interfaces := world.GetHostInterfaces()
	if len(interfaces) != 1 ||
		interfaces[0].GetNamespace() != "wasi" ||
		interfaces[0].GetPackage() != "http" ||
		interfaces[0].GetVersion() != "0.2.0" ||
		interfaces[0].GetName() != "http" ||
		!slices.Equal(interfaces[0].GetInterfaces(), []string{"incoming-handler"}) ||
		interfaces[0].GetConfig()["port"] != "8080" {
		t.Fatal("host interface changed")
	}
}

func TestOlderWorkloadMessagesKeepRestrictiveDefaults(t *testing.T) {
	request := &WorkloadStartRequest{}
	const legacyStart = `{
 "workload": {"name": "legacy", "witWorld": {"components": [{"image": "app:v1"}]}}
}`
	if err := protojson.Unmarshal([]byte(legacyStart), request); err != nil {
		t.Fatal(err)
	}
	w := request.GetWorkload()
	c := w.GetWitWorld().GetComponents()[0]
	if request.GetWorkloadId() != "" ||
		c.GetPoolSize() != 0 ||
		c.GetMaxConcurrency() != 0 ||
		c.GetMaxInvocations() != 0 ||
		c.GetReclaimWindowSeconds() != 0 ||
		c.GetReclaimMinInstances() != 0 {
		t.Fatal("absent instance limits must remain unconfigured")
	}
	if c.GetImagePullSecret().GetUsername() != "" ||
		c.GetImagePullSecret().GetPassword() != "" ||
		c.GetImagePullPolicy() != ImagePullPolicy_IMAGE_PULL_POLICY_UNSPECIFIED {
		t.Fatal("absent image configuration gained credentials or a pull policy")
	}
	r := c.GetLocalResources()
	if len(r.GetAllowedHosts()) != 0 ||
		len(r.GetAllowedIpNameLookups()) != 0 ||
		len(r.GetAllowedHostLoopbackPorts()) != 0 ||
		len(r.GetVolumeMounts()) != 0 {
		t.Fatal("absent resources must grant no network or filesystem access")
	}
	if len(r.GetConfig()) != 0 ||
		len(r.GetEnvironment()) != 0 ||
		r.GetMemoryLimitMb() != 0 ||
		r.GetCpuLimit() != 0 ||
		w.GetService().GetImage() != "" {
		t.Fatal("absent resources or service gained configuration")
	}
	var response *WorkloadStartResponse
	if response.GetWorkloadStatus().GetWorkloadState() != WorkloadState_WORKLOAD_STATE_UNSPECIFIED ||
		response.GetWorkloadStatus().GetWorkloadId() != "" {
		t.Fatal("absent status must not report a running workload")
	}
}
