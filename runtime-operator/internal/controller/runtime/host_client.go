package runtime

import (
	"context"
	"strings"
	"time"

	runtimev2 "go.wasmcloud.dev/runtime-operator/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/pkg/wasmbus"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/emptypb"
)

// HostRoundtripTimeout is the max timeout for host RPC calls.
// Callers can set lower context timeouts as needed.
const HostRoundtripTimeout = 1 * time.Minute

func NewWashHostClient(bus wasmbus.Bus, hostID string) *WashHostClient {
	return &WashHostClient{
		Bus:    bus,
		HostID: hostID,
	}
}

type WashHostClient struct {
	Bus    wasmbus.Bus
	HostID string
}

func (w *WashHostClient) subject(parts ...string) string {
	return strings.Join(append([]string{
		"runtime",
		"host",
		w.HostID,
	}, parts...), ".")
}

func (w *WashHostClient) Heartbeat(ctx context.Context) (*runtimev2.HostHeartbeat, error) {
	var resp runtimev2.HostHeartbeat
	if err := RoundTrip(ctx, w.Bus, w.subject("heartbeat"), &emptypb.Empty{}, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

func (w *WashHostClient) Start(ctx context.Context, req *runtimev2.WorkloadStartRequest) (*runtimev2.WorkloadStartResponse, error) {
	var resp runtimev2.WorkloadStartResponse
	if err := RoundTrip(ctx, w.Bus, w.subject("workload.start"), req, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

func (w *WashHostClient) Status(ctx context.Context, req *runtimev2.WorkloadStatusRequest) (*runtimev2.WorkloadStatusResponse, error) {
	var resp runtimev2.WorkloadStatusResponse
	if err := RoundTrip(ctx, w.Bus, w.subject("workload.status"), req, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

func (w *WashHostClient) Stop(ctx context.Context, req *runtimev2.WorkloadStopRequest) (*runtimev2.WorkloadStopResponse, error) {
	var resp runtimev2.WorkloadStopResponse
	if err := RoundTrip(ctx, w.Bus, w.subject("workload.stop"), req, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// RoundTrip sends a request and waits for a response.
func RoundTrip[Req proto.Message, Resp proto.Message](ctx context.Context, bus wasmbus.Bus, subject string, req Req, resp Resp) error {
	ctx, cancel := context.WithTimeout(ctx, HostRoundtripTimeout)
	defer cancel()

	json, err := protojson.Marshal(req)
	if err != nil {
		return err
	}

	msg := wasmbus.NewMessage(subject)
	msg.Data = json

	reply, err := bus.Request(ctx, msg)
	if err != nil {
		return err
	}

	return protojson.Unmarshal(reply.Data, resp)
}
