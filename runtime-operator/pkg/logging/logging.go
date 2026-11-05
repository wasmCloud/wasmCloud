// Package logging configures log/slog, the standard log package, and
// controller-runtime's logr bridge from one set of command-line flags.
package logging

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

// Options holds the logging flag values. Values set before BindFlags become
// the flag defaults, so each binary can choose its own.
type Options struct {
	// FullObjects logs whole Kubernetes objects instead of their identity.
	FullObjects bool
	// Development defaults to text output, debug logging, and stack traces at warn.
	Development bool
	// JSONLog forces JSON output with numeric epoch timestamps.
	JSONLog         bool
	Format          string
	Level           string
	StacktraceLevel string
	// TimeEncoding defaults to rfc3339 when empty.
	TimeEncoding string
}

// BindFlags registers the logging flags and their zap-named aliases.
func (options *Options) BindFlags(flags *flag.FlagSet) {
	flags.BoolVar(&options.FullObjects, "log-full-objects", options.FullObjects,
		"Include full Kubernetes objects, including Secret contents")
	flags.BoolVar(&options.Development, "dev-mode", options.Development, "Enable development logging defaults")
	flags.BoolVar(&options.JSONLog, "json-log", options.JSONLog, "Output JSON logs with epoch timestamps")
	flags.StringVar(&options.Format, "log-format", options.Format, "Log format: text or json")
	flags.StringVar(&options.Level, "log-level", options.Level,
		"Log level: debug, info, warn, error, panic, or positive verbosity")
	flags.StringVar(&options.StacktraceLevel, "log-stacktrace-level", options.StacktraceLevel,
		"Stacktrace threshold: info, warn, error, or panic")
	flags.StringVar(&options.TimeEncoding, "log-time-encoding", options.TimeEncoding,
		"Timestamp format: epoch, millis, nanos, iso8601, rfc3339, or rfc3339nano")
	flags.BoolVar(&options.Development, "zap-devel", options.Development, "Alias for --dev-mode")
	flags.StringVar(&options.Format, "zap-encoder", options.Format, "Alias for --log-format; console means text")
	flags.StringVar(&options.Level, "zap-log-level", options.Level, "Alias for --log-level")
	flags.StringVar(&options.StacktraceLevel, "zap-stacktrace-level", options.StacktraceLevel,
		"Alias for --log-stacktrace-level")
	flags.StringVar(&options.TimeEncoding, "zap-time-encoding", options.TimeEncoding, "Alias for --log-time-encoding")
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

type config struct {
	level, stackLevel slog.Level
	text              bool
	encoding          string
}

func (options Options) config() (config, error) {
	result := config{level: slog.LevelInfo, stackLevel: slog.LevelError, encoding: "rfc3339"}
	format := logJSON
	if options.Development {
		result.level, result.stackLevel, format = slog.LevelDebug, slog.LevelWarn, logText
	}
	if options.Level != "" {
		var err error
		if result.level, err = logLevel(options.Level); err != nil {
			return config{}, err
		}
	}
	if options.StacktraceLevel != "" {
		switch strings.ToLower(options.StacktraceLevel) {
		case logInfo, logWarn, logError, "panic":
			result.stackLevel, _ = logLevel(options.StacktraceLevel)
		default:
			return config{}, fmt.Errorf("invalid stacktrace level %q", options.StacktraceLevel)
		}
	}
	if options.Format != "" {
		format = strings.ToLower(options.Format)
	}
	if format == "console" {
		format = logText
	}
	if format != logText && format != logJSON {
		return config{}, fmt.Errorf("invalid log format %q", options.Format)
	}
	if options.TimeEncoding != "" {
		result.encoding = strings.ToLower(options.TimeEncoding)
	}
	switch result.encoding {
	case logEpoch, "millis", "nanos", "iso8601", "rfc3339", "rfc3339nano":
	default:
		return config{}, fmt.Errorf("invalid log time encoding %q", options.TimeEncoding)
	}
	// --json-log overrides encoder flags, including timestamp encoding.
	if options.JSONLog {
		format, result.encoding = logJSON, logEpoch
	}
	result.text = format == logText
	return result, nil
}

// Validate reports the error Logger would return for these options.
func (options Options) Validate() error {
	_, err := options.config()
	return err
}

// Logger returns a logger that writes to output in the configured format.
func (options Options) Logger(output io.Writer) (*slog.Logger, error) {
	config, err := options.config()
	if err != nil {
		return nil, err
	}
	handlerOptions := &slog.HandlerOptions{
		Level: config.level, ReplaceAttr: logAttributes(options.FullObjects, config.encoding, config.text),
	}
	var handler slog.Handler = slog.NewJSONHandler(output, handlerOptions)
	if config.text {
		handler = slog.NewTextHandler(output, handlerOptions)
	}
	return slog.New(stacktraceHandler{Handler: handler, level: config.stackLevel}), nil
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

// Configure makes logger the slog default, the standard log output, and the
// controller-runtime logger.
func Configure(logger *slog.Logger) {
	slog.SetDefault(logger)
	log.SetOutput(ErrorLog(logger).Writer())
	ctrl.SetLogger(logr.FromSlogHandler(controllerLogHandler{logger.Handler()}))
}

// ErrorLog returns a standard library logger, such as an http.Server ErrorLog,
// that writes at error level. HTTP diagnostics stay visible at every level
// threshold, without application stack traces.
func ErrorLog(logger *slog.Logger) *log.Logger {
	handler := logger.Handler()
	if stack, ok := handler.(stacktraceHandler); ok {
		stack.skipStack = true
		handler = stack
	}
	return slog.NewLogLogger(handler, slog.LevelError)
}
