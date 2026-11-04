package main

import (
	"fmt"
	"io"
	"log/slog"
	"time"
)

type gatewayLoggingOptions struct {
	level        slog.Level
	format       string
	timeEncoding string
}

func (options gatewayLoggingOptions) validate() error {
	if options.format != "json" && options.format != "text" {
		return fmt.Errorf("invalid log format %q: use json or text", options.format)
	}
	switch options.timeEncoding {
	case "epoch", "rfc3339", "rfc3339nano":
		return nil
	default:
		return fmt.Errorf("invalid log time encoding %q: use epoch, rfc3339, or rfc3339nano", options.timeEncoding)
	}
}

func newGatewayLogger(options gatewayLoggingOptions, output io.Writer) *slog.Logger {
	handlerOptions := &slog.HandlerOptions{Level: options.level, ReplaceAttr: options.replaceAttr}
	if options.format == "text" {
		return slog.New(slog.NewTextHandler(output, handlerOptions))
	}
	return slog.New(slog.NewJSONHandler(output, handlerOptions))
}

func (options gatewayLoggingOptions) replaceAttr(groups []string, attr slog.Attr) slog.Attr {
	if len(groups) != 0 {
		return attr
	}
	// Keep the gateway's JSON field names and default timestamp encoding.
	switch attr.Key {
	case slog.TimeKey:
		if attr.Value.Kind() != slog.KindTime {
			return attr
		}
		timestamp := attr.Value.Time()
		switch options.timeEncoding {
		case "rfc3339":
			return slog.String("ts", timestamp.Format(time.RFC3339))
		case "rfc3339nano":
			return slog.String("ts", timestamp.Format(time.RFC3339Nano))
		default:
			return slog.Float64("ts", float64(timestamp.Unix())+float64(timestamp.Nanosecond())/1e9)
		}
	case slog.LevelKey:
		if level, ok := attr.Value.Any().(slog.Level); ok {
			var label string
			switch {
			case level < slog.LevelInfo:
				label = "debug"
			case level < slog.LevelWarn:
				label = "info"
			case level < slog.LevelError:
				label = "warn"
			default:
				label = "error"
			}
			return slog.String(attr.Key, label)
		}
	case "err":
		attr.Key = "error"
	}
	return attr
}
