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
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/go-logr/logr"
	ctrl "sigs.k8s.io/controller-runtime"
)

const (
	testLogMessage           = "test operator log"
	testProductionFlag       = "--dev-mode=false"
	testLegacyProductionFlag = "--zap-devel=false"
	testJSONFlag             = "--json-log"
	testJSONFormatFlag       = "--log-format=json"
	testWarn                 = "warn"
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
			if got := decodeLog(t, output.Bytes())["ts"]; got != tc.want {
				t.Fatalf("ts = %v (%T), want %v", got, got, tc.want)
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
			commandArgs := append([]string{"-test.run=^TestOperatorLoggingSubprocess$", "--"}, args...)
			command := exec.CommandContext(ctx, os.Args[0], commandArgs...)
			command.Env = append(os.Environ(), "OPERATOR_LOGGING_HELPER=1", "OPERATOR_NAMESPACE=")
			output, err := command.CombinedOutput()
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
		ctrl.SetLogger(logr.Discard())
	})
	for _, level := range []string{"info", testWarn, logError} {
		t.Run(level, func(t *testing.T) {
			var output lockedLogBuffer
			logger, err := parseLogging(t, testJSONFlag, "--log-level="+level).logger(&output)
			if err != nil {
				t.Fatal(err)
			}
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
			if record["level"] != logError || !strings.Contains(record["msg"].(string), "test HTTP panic") {
				t.Fatalf("HTTP diagnostic = %v", record)
			}
		})
	}
}
