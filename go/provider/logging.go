package provider

import (
	"encoding/json"
	"fmt"
	"log/slog"
	"strings"
)

type Level string

const (
	Error    Level = "error"
	Warn     Level = "warn"
	Info     Level = "info"
	Debug    Level = "debug"
	Trace    Level = "trace"
	Critical Level = "critical"
)

func (l Level) String() string {
	return string(l)
}

func (l Level) Level() slog.Level {
	switch l {
	case Error:
		return slog.LevelError
	case Warn:
		return slog.LevelWarn
	case Info:
		return slog.LevelInfo
	case Debug:
		return slog.LevelDebug
	// NOTE: slog doesn't have trace/critical levels so we map them to debug/error
	case Trace:
		return slog.LevelDebug
	case Critical:
		return slog.LevelError
	default:
		return slog.LevelInfo
	}
}

func (l *Level) UnmarshalJSON(data []byte) error {
	var s string
	err := json.Unmarshal(data, &s)
	if err != nil {
		return err
	}

	switch strings.ToLower(s) {
	case "error":
		*l = Error
	case "warn":
		*l = Warn
	case "info":
		*l = Info
	case "debug":
		*l = Debug
	case "trace":
		*l = Trace
	case "critical":
		*l = Critical
	default:
		return fmt.Errorf("invalid level: %s", s)
	}

	return nil
}
