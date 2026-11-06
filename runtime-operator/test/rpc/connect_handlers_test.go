package rpc

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"connectrpc.com/connect"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2/v2connect"
)

func TestServiceRoutesReturnMethodSpecificErrors(t *testing.T) {
	mux := http.NewServeMux()
	workloadPath, workloadHandler := v2connect.NewWorkloadServiceHandler(v2connect.UnimplementedWorkloadServiceHandler{})
	hostPath, hostHandler := v2connect.NewHostServiceHandler(v2connect.UnimplementedHostServiceHandler{})
	mux.Handle(workloadPath, workloadHandler)
	mux.Handle(hostPath, hostHandler)
	server := httptest.NewServer(mux)
	t.Cleanup(server.Close)

	for _, codec := range []string{"protobuf", "json"} {
		t.Run(codec, func(t *testing.T) {
			var options []connect.ClientOption
			if codec == "json" {
				options = append(options, connect.WithProtoJSON())
			}
			workloads := v2connect.NewWorkloadServiceClient(server.Client(), server.URL, options...)
			hosts := v2connect.NewHostServiceClient(server.Client(), server.URL, options...)
			_, startErr := workloads.WorkloadStart(t.Context(), connect.NewRequest(&runtimev2.WorkloadStartRequest{}))
			_, statusErr := workloads.WorkloadStatus(t.Context(), connect.NewRequest(
				&runtimev2.WorkloadStatusRequest{WorkloadId: "workload"}))
			_, stopErr := workloads.WorkloadStop(t.Context(), connect.NewRequest(
				&runtimev2.WorkloadStopRequest{WorkloadId: "workload"}))
			_, heartbeatErr := hosts.HostHeartbeat(t.Context(), connect.NewRequest(&runtimev2.HostHeartbeatRequest{}))
			for method, err := range map[string]error{
				"WorkloadStart": startErr, "WorkloadStatus": statusErr, "WorkloadStop": stopErr, "HostHeartbeat": heartbeatErr,
			} {
				if connect.CodeOf(err) != connect.CodeUnimplemented ||
					!strings.Contains(err.Error(), "."+method+" is not implemented") {
					t.Errorf("%s: %v, want method-specific Unimplemented", method, err)
				}
			}
		})
	}
	for _, path := range []string{workloadPath + "Unknown", hostPath + "Unknown"} {
		response, err := server.Client().Get(server.URL + path)
		if err != nil {
			t.Fatal(err)
		}
		if err := response.Body.Close(); err != nil {
			t.Fatal(err)
		}
		if response.StatusCode != http.StatusNotFound {
			t.Errorf("unknown method %s = %d, want 404", path, response.StatusCode)
		}
	}
}
