// Mostly copied from https://github.com/wrpc/wrpc/blob/main/examples/go/keyvalue-server/cmd/keyvalue-mem-nats/main.go
package main

import (
	"context"
	"sync"

	wrpc "github.com/bytecodealliance/wrpc/go"
	"go.opentelemetry.io/otel/trace"
	"go.wasmcloud.dev/provider"
	"go.wasmcloud.dev/provider/examples/keyvalue-inmemory/bindings/exports/wrpc/keyvalue/store"
)

var (
	errNoSuchStore     = store.NewErrorNoSuchStore()
	errInvalidDataType = store.NewErrorOther("invalid data type stored in map")
)

type Provider struct {
	sync.Map
	sourceLinks map[string]provider.InterfaceLinkDefinition
	targetLinks map[string]provider.InterfaceLinkDefinition
	tracer      trace.Tracer
}

func Ok[T any](v T) *wrpc.Result[T, store.Error] {
	return wrpc.Ok[store.Error](v)
}

func (p *Provider) Delete(ctx context.Context, bucket string, key string) (*wrpc.Result[struct{}, store.Error], error) {
	ctx, span := p.tracer.Start(ctx, "Delete")
	defer span.End()

	v, ok := p.Load(bucket)
	if !ok {
		return wrpc.Err[struct{}](*errNoSuchStore), nil
	}
	b, ok := v.(*sync.Map)
	if !ok {
		return wrpc.Err[struct{}](*errInvalidDataType), nil
	}
	b.Delete(key)
	return Ok(struct{}{}), nil
}

func (p *Provider) Exists(ctx context.Context, bucket string, key string) (*wrpc.Result[bool, store.Error], error) {
	ctx, span := p.tracer.Start(ctx, "Exists")
	defer span.End()

	v, ok := p.Load(bucket)
	if !ok {
		return wrpc.Err[bool](*errNoSuchStore), nil
	}
	b, ok := v.(*sync.Map)
	if !ok {
		return wrpc.Err[bool](*errInvalidDataType), nil
	}
	_, ok = b.Load(key)
	return Ok(ok), nil
}

func (p *Provider) Get(ctx context.Context, bucket string, key string) (*wrpc.Result[[]uint8, store.Error], error) {
	ctx, span := p.tracer.Start(ctx, "Get")
	defer span.End()

	v, ok := p.Load(bucket)
	if !ok {
		return wrpc.Err[[]uint8](*errNoSuchStore), nil
	}
	b, ok := v.(*sync.Map)
	if !ok {
		return wrpc.Err[[]uint8](*errInvalidDataType), nil
	}
	v, ok = b.Load(key)
	if !ok {
		return Ok([]uint8(nil)), nil
	}
	buf, ok := v.([]byte)
	if !ok {
		return wrpc.Err[[]uint8](*errInvalidDataType), nil
	}
	return Ok(buf), nil
}

func (p *Provider) Set(ctx context.Context, bucket string, key string, value []byte) (*wrpc.Result[struct{}, store.Error], error) {
	ctx, span := p.tracer.Start(ctx, "Set")
	defer span.End()

	b := &sync.Map{}
	v, ok := p.LoadOrStore(bucket, b)
	if ok {
		b, ok = v.(*sync.Map)
		if !ok {
			return wrpc.Err[struct{}](*errInvalidDataType), nil
		}
	}
	b.Store(key, value)
	return Ok(struct{}{}), nil
}

func (p *Provider) ListKeys(ctx context.Context, bucket string, cursor *uint64) (*wrpc.Result[store.KeyResponse, store.Error], error) {
	ctx, span := p.tracer.Start(ctx, "ListKeys")
	defer span.End()

	if cursor != nil {
		return wrpc.Err[store.KeyResponse](*store.NewErrorOther("cursors are not supported")), nil
	}
	b := &sync.Map{}
	v, ok := p.LoadOrStore(bucket, b)
	if ok {
		b, ok = v.(*sync.Map)
		if !ok {
			return wrpc.Err[store.KeyResponse](*errInvalidDataType), nil
		}
	}
	var keys []string
	var err *store.Error
	b.Range(func(k, _ any) bool {
		s, ok := k.(string)
		if !ok {
			err = errInvalidDataType
			return false
		}
		keys = append(keys, s)
		return true
	})
	if err != nil {
		return wrpc.Err[store.KeyResponse](*err), nil
	}
	return Ok(store.KeyResponse{
		Keys:   keys,
		Cursor: nil,
	}), nil
}
