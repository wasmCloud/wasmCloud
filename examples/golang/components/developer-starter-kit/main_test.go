//go:generate go run go.wasmcloud.dev/wadge/cmd/wadge-bindgen-go -output bindings.wadge_test.go

package main

import (
	"log"
	"log/slog"
	"os"
)

// NOTE(lxf): Enables full logging, remove timestamp key
func init() {
	log.SetFlags(0)
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
		Level: slog.LevelDebug,
		ReplaceAttr: func(groups []string, a slog.Attr) slog.Attr {
			if a.Key == slog.TimeKey {
				return slog.Attr{}
			}
			return a
		},
	})))
}
