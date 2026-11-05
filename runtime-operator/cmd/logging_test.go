package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"io"
	"log"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/go-logr/logr"
)

const (
	testLogMessage           = "test operator log"
	testProductionFlag       = "--dev-mode=false"
	testLegacyProductionFlag = "--zap-devel=false"
	testJSONFlag             = "--json-log"
	testJSONFormatFlag       = "--log-format=json"
	testWarn                 = "warn"
	testGrouped              = "grouped"
	testPasswordKey          = "password"
)

func parseLogging(t *testing.T, args ...string) loggingOptions {
	t.Helper()
	options := loggingOptions{development: true}
	flags := flag.NewFlagSet("operator", flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	options.bindFlags(flags)
	if err := flags.Parse(args); err != nil {
		t.Fatal(err)
	}
	return options
}

func decodeLog(t *testing.T, data []byte) map[string]any {
	t.Helper()
	var record map[string]any
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.UseNumber()
	if err := decoder.Decode(&record); err != nil {
		t.Fatalf("decode log: %v: %s", err, data)
	}
	return record
}

func TestLoggingFlags(t *testing.T) {
	for _, tc := range []struct {
		name      string
		args      []string
		json      bool
		verbosity int
		stack     bool
	}{
		{name: "defaults", verbosity: 1, stack: true},
		{name: "production", args: []string{testProductionFlag}, json: true},
		{name: "legacy production", args: []string{testLegacyProductionFlag}, json: true},
		{name: "explicit level before mode", args: []string{"--log-level=3", testProductionFlag}, json: true, verbosity: 3},
		{name: "explicit level after mode",
			args: []string{testLegacyProductionFlag, "--zap-log-level=3"}, json: true, verbosity: 3},
		{name: "JSON override", args: []string{testJSONFlag, "--zap-encoder=console"}, json: true, verbosity: 1, stack: true},
		{name: "legacy encoder", args: []string{"--zap-encoder=JSON"}, json: true, verbosity: 1, stack: true},
		{name: "text production", args: []string{testProductionFlag, "--log-format=text"}},
		{name: "explicit stack", args: []string{"--log-stacktrace-level=info"}, verbosity: 1, stack: true},
		{name: "legacy stack", args: []string{"--zap-stacktrace-level=panic"}, verbosity: 1},
		{name: "alias last", args: []string{"--log-level=error", "--zap-log-level=2"}, verbosity: 2, stack: true},
		{name: "neutral last", args: []string{"--zap-log-level=error", "--log-level=2"}, verbosity: 2, stack: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			var output bytes.Buffer
			logger, err := parseLogging(t, tc.args...).logger(&output)
			if err != nil {
				t.Fatal(err)
			}
			controller := logr.FromSlogHandler(controllerLogHandler{logger.Handler()}).
				WithName("controller").WithValues("key", "value")
			for v := 0; v <= 4; v++ {
				if got := controller.V(v).Enabled(); got != (v <= tc.verbosity) {
					t.Errorf("V(%d) enabled = %v", v, got)
				}
			}
			controller.V(tc.verbosity).Info(testLogMessage)
			if output.Len() == 0 {
				t.Fatal("enabled controller record was dropped")
			}
			output.Reset()
			controller.V(tc.verbosity + 1).Info(testLogMessage)
			if output.Len() != 0 {
				t.Fatalf("disabled controller record was logged: %s", output.String())
			}
			logger.Warn(testLogMessage)
			if tc.json {
				record := decodeLog(t, output.Bytes())
				if record["level"] != testWarn || record["msg"] != testLogMessage {
					t.Fatalf("record = %v", record)
				}
			} else if !strings.Contains(output.String(), "level=warn") {
				t.Fatalf("text log = %s", output.String())
			}
			if strings.Contains(output.String(), "stacktrace") != tc.stack {
				t.Fatalf("stacktrace presence: %s", output.String())
			}
		})
	}
}

func TestInvalidLoggingOptions(t *testing.T) {
	for _, arg := range []string{
		"--log-level=bad", "--zap-log-level=0", "--log-level=128", "--log-level=-1", "--log-format=xml",
		"--zap-encoder=bad", "--log-stacktrace-level=debug", "--zap-stacktrace-level=bad", "--log-time-encoding=bad",
	} {
		t.Run(arg, func(t *testing.T) {
			if _, err := parseLogging(t, arg).logger(io.Discard); err == nil {
				t.Fatal("invalid option accepted")
			}
		})
	}
}

func TestLoggingTimestamps(t *testing.T) {
	timestamp := time.Unix(1234567890, 250000000).UTC()
	for _, tc := range []struct {
		args []string
		want any
	}{
		{[]string{testJSONFormatFlag}, "2009-02-13T23:31:30Z"},
		{[]string{testJSONFlag, "--zap-time-encoding=nanos"}, json.Number("1234567890.25")},
		{[]string{"--zap-time-encoding=epoch"}, json.Number("1234567890.25")},
		{[]string{"--log-time-encoding=millis"}, json.Number("1234567890250")},
		{[]string{"--zap-time-encoding=nanos"}, json.Number("1234567890250000000")},
		{[]string{"--log-time-encoding=iso8601"}, "2009-02-13T23:31:30.250Z"},
		{[]string{"--log-time-encoding=rfc3339"}, "2009-02-13T23:31:30Z"},
		{[]string{"--log-time-encoding=rfc3339nano"}, "2009-02-13T23:31:30.25Z"},
	} {
		t.Run(strings.Join(tc.args, " "), func(t *testing.T) {
			args := append([]string{testJSONFormatFlag}, tc.args...)
			var output bytes.Buffer
			logger, err := parseLogging(t, args...).logger(&output)
			if err != nil {
				t.Fatal(err)
			}
			record := slog.NewRecord(timestamp, slog.LevelInfo, testLogMessage, 0)
			if err := logger.Handler().Handle(context.Background(), record); err != nil {
				t.Fatal(err)
			}
			if got := decodeLog(t, output.Bytes())["ts"]; got != tc.want || !bytes.HasPrefix(output.Bytes(), []byte(`{"ts":`)) {
				t.Fatalf("ts = %v (%T), want %v first: %s", got, got, tc.want, output.String())
			}
		})
	}
}

func TestControllerLogging(t *testing.T) {
	for _, level := range []string{"debug", "info", testWarn, logError, "panic", "2"} {
		t.Run(level, func(t *testing.T) {
			var output bytes.Buffer
			logger, err := parseLogging(t, testJSONFlag, "--log-level="+level).logger(&output)
			if err != nil {
				t.Fatal(err)
			}
			controller := logr.FromSlogHandler(controllerLogHandler{logger.Handler()}).
				WithName("setup").WithValues("key", "value")
			controller.Error(errors.New("broken"), testLogMessage)
			if level == "panic" {
				if output.Len() != 0 {
					t.Fatal("error passed panic threshold")
				}
				return
			}
			record := decodeLog(t, output.Bytes())
			if record["level"] != logError || record["logger"] != "setup" ||
				record[logError] != "broken" || record["key"] != "value" || record["stacktrace"] == nil {
				t.Fatalf("controller error = %v", record)
			}
		})
	}
}

func TestOperatorLoggingSubprocess(t *testing.T) {
	if os.Getenv("OPERATOR_LOGGING_HELPER") == "1" {
		for i, arg := range os.Args {
			if arg == "--" {
				os.Args = append([]string{os.Args[0]}, os.Args[i+1:]...)
				break
			}
		}
		main()
		os.Exit(0)
	}
	for _, args := range [][]string{
		{testJSONFlag, "--log-level=error"},
		{testLegacyProductionFlag, "--zap-log-level=error", "--zap-time-encoding=epoch"},
		{testJSONFormatFlag, "--log-stacktrace-level=panic"},
		{"--log-format=bad"},
	} {
		t.Run(strings.Join(args, " "), func(t *testing.T) {
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			config := filepath.Join(t.TempDir(), "invalid-kubeconfig")
			if err := os.WriteFile(config, []byte("apiVersion: [invalid"), 0600); err != nil {
				t.Fatal(err)
			}
			commandArgs := append([]string{"-test.run=^TestOperatorLoggingSubprocess$", "--",
				"--nats-url=nats://127.0.0.1:1"}, args...)
			command := exec.CommandContext(ctx, os.Args[0], commandArgs...)
			command.Env = append(os.Environ(), "OPERATOR_LOGGING_HELPER=1", "OPERATOR_NAMESPACE=", "KUBECONFIG="+config)
			output, err := command.CombinedOutput()
			if ctx.Err() != nil {
				t.Fatalf("startup did not reject missing namespace before timeout: %v: %s", ctx.Err(), output)
			}
			var exit *exec.ExitError
			if !errors.As(err, &exit) {
				t.Fatalf("process = %v: %s", err, output)
			}
			if args[0] == "--log-format=bad" {
				if exit.ExitCode() != 2 || !strings.Contains(string(output), "invalid log format") {
					t.Fatalf("invalid flags: %v: %s", err, output)
				}
				return
			}
			if exit.ExitCode() != 1 {
				t.Fatalf("exit = %d: %s", exit.ExitCode(), output)
			}
			record := decodeLog(t, output)
			if record["level"] != logError || record["logger"] != "setup" ||
				record[logError] != "missing required configuration: OPERATOR_NAMESPACE is unset" || record["ts"] == nil {
				t.Fatalf("startup error = %v", record)
			}
			if args[0] == testJSONFlag {
				if _, ok := record["ts"].(json.Number); !ok {
					t.Fatalf("epoch timestamp = %v", record["ts"])
				}
			}
		})
	}
}

type lockedLogBuffer struct {
	sync.Mutex
	buffer bytes.Buffer
}

func (buffer *lockedLogBuffer) Write(data []byte) (int, error) {
	buffer.Lock()
	defer buffer.Unlock()
	return buffer.buffer.Write(data)
}
func (buffer *lockedLogBuffer) bytes() []byte {
	buffer.Lock()
	defer buffer.Unlock()
	return bytes.Clone(buffer.buffer.Bytes())
}

func TestStandardHTTPDiagnostics(t *testing.T) {
	oldLogger, oldWriter, oldFlags := slog.Default(), log.Writer(), log.Flags()
	oldLevel := slog.SetLogLoggerLevel(slog.LevelInfo)
	t.Cleanup(func() {
		slog.SetDefault(oldLogger)
		slog.SetLogLoggerLevel(oldLevel)
		log.SetOutput(oldWriter)
		log.SetFlags(oldFlags)
	})
	for _, level := range []string{"info", testWarn, logError} {
		t.Run(level, func(t *testing.T) {
			var output lockedLogBuffer
			logger, err := parseLogging(t, testJSONFlag, "--log-level="+level).logger(&output)
			if err != nil {
				t.Fatal(err)
			}
			// controller-runtime keeps only the process's first SetLogger, so controller logs stay on the first buffer.
			configureLogging(logger)
			server := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) { panic("test HTTP panic") }))
			defer server.Close()
			response, err := server.Client().Get(server.URL)
			if response != nil {
				if err := response.Body.Close(); err != nil {
					t.Fatal(err)
				}
			}
			if err == nil {
				t.Fatal("panic request succeeded")
			}
			record := decodeLog(t, output.bytes())
			if record["stacktrace"] != nil {
				t.Fatalf("standard HTTP diagnostic gained a logging stack: %v", record)
			}
			if record["level"] != logError || !strings.Contains(record["msg"].(string), "test HTTP panic") {
				t.Fatalf("HTTP diagnostic = %v", record)
			}
		})
	}
}

func TestTextTimestamps(t *testing.T) {
	for _, tc := range []struct{ encoding, want string }{
		{"epoch", "ts=1234567890.25"}, {"millis", "ts=1234567890250"}, {"rfc3339", "ts=2009-02-13T23:31:30Z"},
	} {
		var output bytes.Buffer
		logger, err := parseLogging(t, "--log-format=text", "--log-time-encoding="+tc.encoding).logger(&output)
		if err != nil {
			t.Fatal(err)
		}
		record := slog.NewRecord(time.Unix(1234567890, 250000000).UTC(), slog.LevelInfo, testLogMessage, 0)
		if err := logger.Handler().Handle(t.Context(), record); err != nil {
			t.Fatal(err)
		}
		if !strings.HasPrefix(output.String(), tc.want+" level=info msg=") {
			t.Fatalf("timestamp = %s", output.String())
		}
	}
}

type timeLogValuer time.Time

func (value timeLogValuer) LogValue() slog.Value { return slog.TimeValue(time.Time(value)) }

func TestTimestampDoesNotRenameUserTimeFields(t *testing.T) {
	timestamp := time.Unix(1234567890, 250000000).UTC()
	userTime := timestamp.Add(time.Hour)
	controller := func(logger *slog.Logger) logr.Logger {
		return logr.FromSlogHandler(controllerLogHandler{logger.Handler()})
	}
	for _, tc := range []struct {
		name  string
		group bool
		emit  func(*slog.Logger)
	}{
		{"inline", false, func(logger *slog.Logger) { logger.Info(testLogMessage, "time", userTime) }},
		{"bound", false, func(logger *slog.Logger) { logger.With("time", userTime).Info(testLogMessage) }},
		{"inline group", false, func(logger *slog.Logger) {
			logger.Info(testLogMessage, slog.Group("", slog.Time("time", userTime)))
		}},
		{"valuer", false, func(logger *slog.Logger) { logger.Info(testLogMessage, "time", timeLogValuer(userTime)) }},
		{"marshaler", false, func(logger *slog.Logger) {
			logger.Info(testLogMessage, "time", objectLogMarshaler{userTime})
		}},
		{"controller inline", false, func(logger *slog.Logger) { controller(logger).Info(testLogMessage, "time", userTime) }},
		{"controller bound", false, func(logger *slog.Logger) {
			controller(logger).WithValues("time", userTime).Info(testLogMessage)
		}},
		{testGrouped, true, func(logger *slog.Logger) {
			logger.WithGroup(testPayload).With("time", userTime).Info(testLogMessage)
		}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			var output bytes.Buffer
			logger, err := parseLogging(t, testJSONFlag).logger(&output)
			if err != nil {
				t.Fatal(err)
			}
			tc.emit(logger)
			got := decodeLog(t, output.Bytes())
			if _, ok := got["ts"].(json.Number); !ok || bytes.Count(output.Bytes(), []byte(`"ts":`)) != 1 {
				t.Fatalf("built-in timestamp = %s", output.String())
			}
			if tc.group {
				got = got[testPayload].(map[string]any)
			}
			if got["time"] != userTime.Format(time.RFC3339Nano) {
				t.Fatalf("user time field = %s", output.String())
			}
		})
	}
}

type countingLogValuer struct{ calls *atomic.Int32 }

func (value countingLogValuer) LogValue() slog.Value {
	value.calls.Add(1)
	return slog.StringValue(testLogMessage)
}

func TestBoundFieldsResolveOnce(t *testing.T) {
	var calls atomic.Int32
	value := countingLogValuer{&calls}
	logger, err := parseLogging(t, testJSONFlag).logger(io.Discard)
	if err != nil {
		t.Fatal(err)
	}
	bound := []func(string, ...any){
		logger.With("field", value).Info,
		logger.With("time", value).Info,
		logger.WithGroup(testPayload).With("field", value).Info,
		logr.FromSlogHandler(controllerLogHandler{logger.Handler()}).WithValues("field", value).Info,
	}
	for range 3 {
		for _, info := range bound {
			info(testLogMessage)
		}
	}
	if got := calls.Load(); got != int32(len(bound)) {
		t.Fatalf("bound values resolved %d times, want once per binding (%d)", got, len(bound))
	}
}

func emitOperatorError(logger *slog.Logger, controller bool) {
	if controller {
		logr.FromSlogHandler(controllerLogHandler{logger.Handler()}).Error(errors.New("stack test"), testLogMessage)
	} else {
		logger.Error(testLogMessage)
	}
}

func TestApplicationStackStartsAtLogCall(t *testing.T) {
	for _, controller := range []bool{false, true} {
		var output bytes.Buffer
		logger, err := parseLogging(t, testJSONFlag).logger(&output)
		if err != nil {
			t.Fatal(err)
		}
		emitOperatorError(logger, controller)
		record := decodeLog(t, output.Bytes())
		stack, ok := record["stacktrace"].(string)
		if !ok || !strings.Contains(strings.Split(stack, "\n")[0], ".emitOperatorError") {
			t.Fatalf("stack does not start at log call: %v", record)
		}
		for _, internal := range []string{"stacktraceHandler", "log/slog.", "slogSink"} {
			if strings.Contains(stack, internal) {
				t.Fatalf("stack contains logging frame %s: %s", internal, stack)
			}
		}
	}
}

func TestControllerLogsRetainVerbosity(t *testing.T) {
	for verbosity := 0; verbosity <= 3; verbosity++ {
		var output bytes.Buffer
		logger, err := parseLogging(t, testJSONFlag, "--log-level=3").logger(&output)
		if err != nil {
			t.Fatal(err)
		}
		logr.FromSlogHandler(controllerLogHandler{logger.Handler()}).V(verbosity).Info(testLogMessage)
		if got := decodeLog(t, output.Bytes())["v"]; got != json.Number(strconv.Itoa(verbosity)) {
			t.Fatalf("verbosity %d logged as %v", verbosity, got)
		}
	}
}
