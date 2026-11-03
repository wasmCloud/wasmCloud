package wasmbus

import (
	"context"
	"errors"
	"fmt"
	"testing"
)

func TestRequestHandlerFreshPopulatedDefaults(t *testing.T) {
	const defaultName = "default"
	type nested struct {
		Name string `json:"name" yaml:"name"`
	}
	type request struct {
		Labels map[string]string `json:"labels" yaml:"labels"`
		Values []string          `json:"values" yaml:"values"`
		Nested *nested           `json:"nested" yaml:"nested"`
		Local  map[string]string `json:"-" yaml:"-"`
	}
	for _, mimeType := range []string{mimetypeJSON, mimeTypeYAML} {
		t.Run(mimeType, func(t *testing.T) {
			allocations := 0
			var requests []*request
			wantName := ""
			handler := NewRequestHandler(func() request {
				allocations++
				return request{
					Labels: map[string]string{"name": defaultName},
					Values: []string{defaultName},
					Nested: &nested{Name: defaultName},
					Local:  map[string]string{"name": "local"},
				}
			}, testResponse{}, func(_ context.Context, req *request) (*testResponse, error) {
				if req.Local["name"] != "prepared" {
					t.Fatalf("handler lost the pre-hook mutation: %+v", req.Local)
				}
				changed := fmt.Sprintf("changed-%d", len(requests))
				req.Labels["name"] = changed
				req.Values[0] = changed
				req.Nested.Name = changed
				req.Local["name"] = changed
				requests = append(requests, req)
				return &testResponse{}, nil
			})
			handler.PreRequest = func(_ context.Context, req *request, _ *Message) error {
				if req.Labels["name"] != wantName ||
					len(req.Values) != 1 || req.Values[0] != wantName || req.Nested == nil || req.Nested.Name != wantName ||
					req.Local["name"] != "local" {
					t.Fatalf("request defaults or decoded values = %+v, want %q with fresh local defaults", req, wantName)
				}
				req.Local["name"] = "prepared"
				return nil
			}
			bus := &handlerTestBus{}
			for _, tc := range []struct {
				data string
				name string
				err  error
			}{
				{`{"labels":{"name":"first"},"values":["first"],"nested":{"name":"first"}}`, "first", nil},
				{`{"labels":{"name":"leaked"},"nested":123}`, "", ErrDecode},
				{`{}`, defaultName, nil},
				{"", defaultName, nil},
			} {
				wantName = tc.name
				msg := NewMessage(testSubject)
				msg.Reply = handlerReplySubject
				msg.Header.Set("Content-Type", mimeType)
				msg.Data = []byte(tc.data)
				msg.bus = bus
				if err := handler.HandleMessage(t.Context(), msg); !errors.Is(err, tc.err) {
					t.Fatalf("HandleMessage(%q) = %v, want %v", tc.data, err, tc.err)
				}
			}
			if allocations != 4 || len(requests) != 3 || bus.publishCalls != 3 {
				t.Fatalf("allocations, handled requests, publishes = %d, %d, %d; want 4, 3, 3",
					allocations, len(requests), bus.publishCalls)
			}
			for i, req := range requests {
				want := fmt.Sprintf("changed-%d", i)
				if req.Labels["name"] != want || req.Values[0] != want || req.Nested.Name != want || req.Local["name"] != want {
					t.Fatalf("request %d shares mutable defaults with a later request: %+v", i, req)
				}
			}
		})
	}
}

func TestRequestHandlerNilFactoryUsesZeroValue(t *testing.T) {
	handler := NewRequestHandler(nil, testResponse{}, func(_ context.Context, req *testMessage) (*testResponse, error) {
		if *req != (testMessage{}) {
			t.Fatalf("request = %+v, want zero value", req)
		}
		req.Name = "changed"
		return &testResponse{}, nil
	})
	for range 2 {
		msg := NewMessage(testSubject)
		msg.Reply = handlerReplySubject
		msg.bus = &handlerTestBus{}
		if err := handler.HandleMessage(t.Context(), msg); err != nil {
			t.Fatal(err)
		}
	}
}
