package main

import (
	"context"
	"flag"
	"fmt"
	"io"
	"log/slog"
	"runtime/debug"
	"strconv"
	"strings"
	"time"

	"github.com/go-logr/logr"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	ctrl "sigs.k8s.io/controller-runtime"
)

const (
	logError = "error"
	logJSON  = "json"
	logText  = "text"
	logEpoch = "epoch"
	logInfo  = "info"
	logWarn  = "warn"
)

type loggingOptions struct {
	development bool
	jsonLog     bool
	format      string
	level       string
	stackLevel  string
	timeEncoder string
}

func (options *loggingOptions) bindFlags(flags *flag.FlagSet) {
	flags.BoolVar(&options.development, "dev-mode", options.development, "Enable development logging defaults")
	flags.BoolVar(&options.jsonLog, "json-log", false, "Output JSON logs with epoch timestamps")
	flags.StringVar(&options.format, "log-format", "", "Log format: text or json")
	flags.StringVar(&options.level, "log-level", "", "Log level: debug, info, warn, error, panic, or positive verbosity")
	flags.StringVar(&options.stackLevel, "log-stacktrace-level", "", "Stacktrace threshold: info, warn, error, or panic")
	flags.StringVar(&options.timeEncoder, "log-time-encoding", "",
		"Timestamp format: epoch, millis, nanos, iso8601, rfc3339, or rfc3339nano")
	flags.BoolVar(&options.development, "zap-devel", options.development, "Alias for --dev-mode")
	flags.StringVar(&options.format, "zap-encoder", "", "Alias for --log-format; console means text")
	flags.StringVar(&options.level, "zap-log-level", "", "Alias for --log-level")
	flags.StringVar(&options.stackLevel, "zap-stacktrace-level", "", "Alias for --log-stacktrace-level")
	flags.StringVar(&options.timeEncoder, "zap-time-encoding", "", "Alias for --log-time-encoding")
}

func logLevel(value string) (slog.Level, error) {
	switch strings.ToLower(value) {
	case "debug":
		return slog.LevelDebug, nil
	case logInfo:
		return slog.LevelInfo, nil
	case logWarn:
		return slog.LevelWarn, nil
	case logError:
		return slog.LevelError, nil
	case "panic":
		return 16, nil
	default:
		verbosity, err := strconv.Atoi(value)
		if err != nil || verbosity < 1 || verbosity > 127 {
			return 0, fmt.Errorf("invalid log level %q", value)
		}
		return -slog.Level(verbosity) * 4, nil
	}
}

func (options loggingOptions) logger(output io.Writer) (*slog.Logger, error) {
	level, stackLevel := slog.LevelInfo, slog.LevelError
	format := logJSON
	if options.development {
		level, stackLevel, format = slog.LevelDebug, slog.LevelWarn, logText
	}
	if options.level != "" {
		var err error
		level, err = logLevel(options.level)
		if err != nil {
			return nil, err
		}
	}
	if options.stackLevel != "" {
		switch strings.ToLower(options.stackLevel) {
		case logInfo, logWarn, logError, "panic":
			stackLevel, _ = logLevel(options.stackLevel)
		default:
			return nil, fmt.Errorf("invalid stacktrace level %q", options.stackLevel)
		}
	}
	if options.format != "" {
		format = strings.ToLower(options.format)
	}
	if format == "console" {
		format = logText
	}
	if format != logText && format != logJSON {
		return nil, fmt.Errorf("invalid log format %q", options.format)
	}
	encoding := strings.ToLower(options.timeEncoder)
	if encoding == "" {
		encoding = "rfc3339"
	}
	switch encoding {
	case logEpoch, "millis", "nanos", "iso8601", "rfc3339", "rfc3339nano":
	default:
		return nil, fmt.Errorf("invalid log time encoding %q", options.timeEncoder)
	}
	// --json-log overrides encoder flags, including timestamp encoding.
	if options.jsonLog {
		format, encoding = logJSON, logEpoch
	}
	handlerOptions := &slog.HandlerOptions{Level: level, ReplaceAttr: logAttributes(encoding, options.development)}
	var handler slog.Handler = slog.NewJSONHandler(output, handlerOptions)
	if format == logText {
		handler = slog.NewTextHandler(output, handlerOptions)
	}
	return slog.New(stacktraceHandler{Handler: handler, level: stackLevel}), nil
}

type marshalerValue struct{ logr.Marshaler }

func (value marshalerValue) LogValue() slog.Value { return slog.AnyValue(value.MarshalLog()) }

type kubernetesLogObject interface {
	metav1.Object
	runtime.Object
}

type objectLogValue struct{ kubernetesLogObject }

func (value objectLogValue) LogValue() slog.Value {
	// Production logs contain object identity, not object contents.
	attrs := []slog.Attr{slog.String("name", value.GetName())}
	if namespace := value.GetNamespace(); namespace != "" {
		attrs = append(attrs, slog.String("namespace", namespace))
	}
	if gvk := value.GetObjectKind().GroupVersionKind(); gvk.Version != "" {
		attrs = append(attrs, slog.String("apiVersion", gvk.GroupVersion().String()), slog.String("kind", gvk.Kind))
	}
	return slog.GroupValue(attrs...)
}

func logAttributes(encoding string, verbose bool) func([]string, slog.Attr) slog.Attr {
	return func(groups []string, attr slog.Attr) slog.Attr {
		if marshaler, ok := attr.Value.Any().(logr.Marshaler); ok {
			attr.Value = slog.AnyValue(marshalerValue{marshaler}).Resolve()
		} else if !verbose {
			if object, ok := attr.Value.Any().(kubernetesLogObject); ok {
				attr.Value = slog.AnyValue(objectLogValue{object}).Resolve()
			}
		}
		if len(groups) != 0 {
			return attr
		}
		switch attr.Key {
		case slog.TimeKey:
			if attr.Value.Kind() == slog.KindTime {
				return timestampAttribute(encoding, attr.Value.Time())
			}
		case slog.LevelKey:
			if level, ok := attr.Value.Any().(slog.Level); ok {
				label := "debug"
				switch {
				case level >= slog.LevelError:
					label = logError
				case level >= slog.LevelWarn:
					label = logWarn
				case level >= slog.LevelInfo:
					label = logInfo
				}
				return slog.String(attr.Key, label)
			}
		case "err":
			attr.Key = logError
		}
		return attr
	}
}

func timestampAttribute(encoding string, timestamp time.Time) slog.Attr {
	switch encoding {
	case logEpoch:
		return slog.Float64("ts", float64(timestamp.Unix())+float64(timestamp.Nanosecond())/1e9)
	case "millis":
		return slog.Float64("ts", float64(timestamp.Unix())*1e3+float64(timestamp.Nanosecond())/1e6)
	case "nanos":
		return slog.Int64("ts", timestamp.UnixNano())
	case "iso8601":
		return slog.String("ts", timestamp.Format("2006-01-02T15:04:05.000Z0700"))
	case "rfc3339nano":
		return slog.String("ts", timestamp.Format(time.RFC3339Nano))
	default:
		return slog.String("ts", timestamp.Format(time.RFC3339))
	}
}

type stacktraceHandler struct {
	slog.Handler
	level slog.Level
}

func (handler stacktraceHandler) Handle(ctx context.Context, record slog.Record) error {
	if record.Level >= handler.level {
		record = record.Clone()
		record.AddAttrs(slog.String("stacktrace", string(debug.Stack())))
	}
	return handler.Handler.Handle(ctx, record)
}

func (handler stacktraceHandler) WithAttrs(attrs []slog.Attr) slog.Handler {
	handler.Handler = handler.Handler.WithAttrs(attrs)
	return handler
}

func (handler stacktraceHandler) WithGroup(name string) slog.Handler {
	handler.Handler = handler.Handler.WithGroup(name)
	return handler
}

// One logr verbosity step equals one slog severity step below Info.
type controllerLogHandler struct{ slog.Handler }

func controllerLogLevel(level slog.Level) slog.Level {
	if level < 0 {
		return level * 4
	}
	return level
}

func (handler controllerLogHandler) Enabled(ctx context.Context, level slog.Level) bool {
	return handler.Handler.Enabled(ctx, controllerLogLevel(level))
}

func (handler controllerLogHandler) Handle(ctx context.Context, record slog.Record) error {
	record.Level = controllerLogLevel(record.Level)
	// logr.Error calls Handle without checking Enabled.
	if !handler.Handler.Enabled(ctx, record.Level) {
		return nil
	}
	return handler.Handler.Handle(ctx, record)
}

func (handler controllerLogHandler) WithAttrs(attrs []slog.Attr) slog.Handler {
	return controllerLogHandler{handler.Handler.WithAttrs(attrs)}
}

func (handler controllerLogHandler) WithGroup(name string) slog.Handler {
	return controllerLogHandler{handler.Handler.WithGroup(name)}
}

func configureLogging(logger *slog.Logger) {
	slog.SetDefault(logger)
	// Controller-runtime's HTTP servers use the standard logger for errors.
	slog.SetLogLoggerLevel(slog.LevelError)
	ctrl.SetLogger(logr.FromSlogHandler(controllerLogHandler{logger.Handler()}))
}
