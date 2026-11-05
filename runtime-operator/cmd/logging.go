package main

import (
	"context"
	"flag"
	"fmt"
	"io"
	"log"
	"log/slog"
	"reflect"
	goruntime "runtime"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/go-logr/logr"
	"k8s.io/apimachinery/pkg/api/meta"
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
	fullObjects bool
	development bool
	jsonLog     bool
	format      string
	level       string
	stackLevel  string
	timeEncoder string
}

func (options *loggingOptions) bindFlags(flags *flag.FlagSet) {
	flags.BoolVar(&options.fullObjects, "log-full-objects", false,
		"Include full Kubernetes objects, including Secret contents")
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
	handlerOptions := &slog.HandlerOptions{
		Level: level, ReplaceAttr: logAttributes(options.fullObjects, encoding, format == logText),
	}
	var handler slog.Handler = slog.NewJSONHandler(output, handlerOptions)
	if format == logText {
		handler = slog.NewTextHandler(output, handlerOptions)
	}
	return slog.New(stacktraceHandler{Handler: handler, level: stackLevel}), nil
}

func nilPointer(value any) bool {
	reflected := reflect.ValueOf(value)
	return reflected.Kind() == reflect.Pointer && reflected.IsNil()
}

type marshalerValue struct{ logr.Marshaler }

func (value marshalerValue) LogValue() slog.Value {
	// Nil pointers log as null instead of panicking in MarshalLog.
	if nilPointer(value.Marshaler) {
		return slog.AnyValue(nil)
	}
	return slog.AnyValue(value.MarshalLog())
}

type objectLogValue struct{ runtime.Object }

func (value objectLogValue) LogValue() slog.Value {
	if nilPointer(value.Object) {
		return slog.AnyValue(nil)
	}
	// Object logs contain identity unless full contents are explicitly enabled.
	var attrs []slog.Attr
	if metadata, err := meta.Accessor(value.Object); err == nil {
		attrs = append(attrs, slog.String("name", metadata.GetName()))
		if namespace := metadata.GetNamespace(); namespace != "" {
			attrs = append(attrs, slog.String("namespace", namespace))
		}
	}
	if gvk := value.GetObjectKind().GroupVersionKind(); gvk.Version != "" {
		attrs = append(attrs, slog.String("apiVersion", gvk.GroupVersion().String()), slog.String("kind", gvk.Kind))
	}
	return slog.GroupValue(attrs...)
}

// Value.Any boxes scalar kinds into a new interface, so only KindAny values are inspected.
func logValue(value slog.Value, fullObjects bool) slog.Value {
	if value.Kind() != slog.KindAny {
		return value
	}
	if marshaler, ok := value.Any().(logr.Marshaler); ok {
		value = slog.AnyValue(marshalerValue{marshaler}).Resolve()
	}
	if !fullObjects && value.Kind() == slog.KindAny {
		if object, ok := value.Any().(runtime.Object); ok {
			value = slog.AnyValue(objectLogValue{object}).Resolve()
		}
	}
	return value
}

func logAttributes(fullObjects bool, timeEncoder string, text bool) func([]string, slog.Attr) slog.Attr {
	return func(groups []string, attr slog.Attr) slog.Attr {
		if attr.Value.Kind() == slog.KindAny {
			if field, ok := attr.Value.Any().(timeField); ok {
				attr.Value = logValue(field.value.Resolve(), fullObjects)
				return attr
			}
		}
		attr.Value = logValue(attr.Value, fullObjects)
		if len(groups) != 0 {
			return attr
		}
		switch attr.Key {
		case slog.TimeKey:
			// User time fields arrive as timeField, so this is the built-in timestamp.
			if attr.Value.Kind() == slog.KindTime {
				return timestampAttribute(timeEncoder, text, attr.Value.Time())
			}
		case slog.LevelKey:
			if attr.Value.Kind() != slog.KindAny {
				break
			}
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

func timestampAttribute(encoding string, text bool, timestamp time.Time) slog.Attr {
	switch encoding {
	case logEpoch:
		return numericTimestamp(text, float64(timestamp.Unix())+float64(timestamp.Nanosecond())/1e9)
	case "millis":
		return numericTimestamp(text, float64(timestamp.Unix())*1e3+float64(timestamp.Nanosecond())/1e6)
	case "nanos":
		return slog.Int64("ts", timestamp.UnixNano())
	case "iso8601":
		return slog.String("ts", timestamp.Format("2006-01-02T15:04:05.000Z0700"))
	case "rfc3339nano":
		// JSON writes time values as RFC3339Nano directly into its buffer.
		if !text {
			return slog.Time("ts", timestamp)
		}
		return slog.String("ts", timestamp.Format(time.RFC3339Nano))
	default:
		return slog.String("ts", timestamp.Format(time.RFC3339))
	}
}

// Text output writes floats in exponent form, so text timestamps are plain decimal strings.
func numericTimestamp(text bool, value float64) slog.Attr {
	if text {
		return slog.String("ts", strconv.FormatFloat(value, 'f', -1, 64))
	}
	return slog.Float64("ts", value)
}

// timeField wraps a user attribute named time that lands outside every group,
// so ReplaceAttr renames only the built-in timestamp to ts.
type timeField struct{ value slog.Value }

func isTimeField(attr slog.Attr) bool {
	if attr.Key == "" && attr.Value.Kind() == slog.KindGroup {
		return slices.ContainsFunc(attr.Value.Group(), isTimeField)
	}
	return attr.Key == slog.TimeKey && attr.Value.Kind() != slog.KindGroup
}

func markTimeField(attr slog.Attr) slog.Attr {
	switch {
	case !isTimeField(attr):
		return attr
	case attr.Key == "":
		return slog.Attr{Value: slog.GroupValue(markTimeFields(attr.Value.Group())...)}
	default:
		return slog.Any(attr.Key, timeField{attr.Value})
	}
}

func markTimeFields(attrs []slog.Attr) []slog.Attr {
	if !slices.ContainsFunc(attrs, isTimeField) {
		return attrs
	}
	marked := make([]slog.Attr, len(attrs))
	for i, attr := range attrs {
		marked[i] = markTimeField(attr)
	}
	return marked
}

func markRecordTimeFields(record slog.Record) slog.Record {
	found := false
	record.Attrs(func(attr slog.Attr) bool {
		found = isTimeField(attr)
		return !found
	})
	if !found {
		return record
	}
	marked := slog.NewRecord(record.Time, record.Level, record.Message, record.PC)
	record.Attrs(func(attr slog.Attr) bool {
		marked.AddAttrs(markTimeField(attr))
		return true
	})
	return marked
}

type stacktraceHandler struct {
	slog.Handler
	level     slog.Level
	skipStack bool
	grouped   bool
}

func (handler stacktraceHandler) Handle(ctx context.Context, record slog.Record) error {
	if !handler.grouped {
		record = markRecordTimeFields(record)
	}
	if !handler.skipStack && record.Level >= handler.level {
		if stack := logStack(record.PC); stack != "" {
			record = record.Clone()
			record.AddAttrs(slog.String("stacktrace", stack))
		}
	}
	return handler.Handler.Handle(ctx, record)
}

func logStack(pc uintptr) string {
	if pc == 0 {
		return ""
	}
	origin, _ := goruntime.CallersFrames([]uintptr{pc}).Next()
	var pcs [64]uintptr
	count := goruntime.Callers(2, pcs[:])
	frames := goruntime.CallersFrames(pcs[:count])
	var output strings.Builder
	found := false
	for {
		frame, more := frames.Next()
		if frame.Function == origin.Function && frame.File == origin.File && frame.Line == origin.Line {
			found = true
		}
		if found {
			fmt.Fprintf(&output, "%s\n\t%s:%d\n", frame.Function, frame.File, frame.Line)
		}
		if !more {
			break
		}
	}
	if !found {
		fmt.Fprintf(&output, "%s\n\t%s:%d\n", origin.Function, origin.File, origin.Line)
	}
	return strings.TrimSuffix(output.String(), "\n")
}

func (handler stacktraceHandler) WithAttrs(attrs []slog.Attr) slog.Handler {
	if !handler.grouped {
		attrs = markTimeFields(attrs)
	}
	handler.Handler = handler.Handler.WithAttrs(attrs)
	return handler
}

func (handler stacktraceHandler) WithGroup(name string) slog.Handler {
	handler.grouped = handler.grouped || name != ""
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
	if record.Level <= 0 {
		record = record.Clone()
		record.AddAttrs(slog.Int("v", -int(record.Level)))
	}
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
	// HTTP diagnostics stay visible at error thresholds without application stack traces.
	handler := logger.Handler()
	if stack, ok := handler.(stacktraceHandler); ok {
		stack.skipStack = true
		handler = stack
	}
	log.SetOutput(slog.NewLogLogger(handler, slog.LevelError).Writer())
	ctrl.SetLogger(logr.FromSlogHandler(controllerLogHandler{logger.Handler()}))
}
