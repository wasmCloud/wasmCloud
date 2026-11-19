package logging

import (
	"bytes"
	"context"
	"log/slog"
	goruntime "runtime"
	"strings"
	"testing"
	"time"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

// Each explicit stacktrace level attaches stacks at that severity and above,
// and nowhere below it.
func TestStacktraceLevelThreshold(t *testing.T) {
	levels := []slog.Level{slog.LevelInfo, slog.LevelWarn, slog.LevelError}
	for _, tc := range []struct {
		flag      string
		threshold slog.Level
	}{
		{logInfo, slog.LevelInfo},
		{"WARN", slog.LevelWarn},
		{logError, slog.LevelError},
		{"Panic", slog.LevelError + 4},
	} {
		t.Run(tc.flag, func(t *testing.T) {
			for _, level := range levels {
				var output bytes.Buffer
				logger, err := parseLogging(t, testJSONFlag, "--log-level=info", "--log-stacktrace-level="+tc.flag).Logger(&output)
				if err != nil {
					t.Fatal(err)
				}
				logger.Log(context.Background(), level, testLogMessage)
				_, hasStack := decodeLog(t, output.Bytes())["stacktrace"]
				if want := level >= tc.threshold; hasStack != want {
					t.Errorf("level %v: stacktrace present = %v, want %v", level, hasStack, want)
				}
			}
		})
	}
}

// A record with no caller (built by hand, or from an adapter that drops the
// PC) has no stack to report, even at a level that would carry one.
func TestStacktraceWithoutCaller(t *testing.T) {
	var output bytes.Buffer
	logger, err := parseLogging(t, testJSONFlag).Logger(&output)
	if err != nil {
		t.Fatal(err)
	}
	record := slog.NewRecord(time.Now(), slog.LevelError, testLogMessage, 0)
	if err := logger.Handler().Handle(context.Background(), record); err != nil {
		t.Fatal(err)
	}
	if record := decodeLog(t, output.Bytes()); record["stacktrace"] != nil {
		t.Fatalf("stacktrace without a caller: %v", record)
	}
}

func capturedPC() uintptr {
	var pcs [1]uintptr
	goruntime.Callers(1, pcs[:])
	return pcs[0]
}

// A record handled after its caller returned (queued, or replayed from another
// goroutine) cannot be found on the current stack, so the stack falls back to
// the call site alone rather than to the unrelated frames handling it.
func TestStacktraceForDetachedRecord(t *testing.T) {
	var output bytes.Buffer
	logger, err := parseLogging(t, testJSONFlag).Logger(&output)
	if err != nil {
		t.Fatal(err)
	}
	record := slog.NewRecord(time.Now(), slog.LevelError, testLogMessage, capturedPC())
	if err := logger.Handler().Handle(context.Background(), record); err != nil {
		t.Fatal(err)
	}
	stack, _ := decodeLog(t, output.Bytes())["stacktrace"].(string)
	lines := strings.Split(stack, "\n")
	if len(lines) != 2 || !strings.HasSuffix(lines[0], ".capturedPC") ||
		!strings.Contains(lines[1], "stacktrace_test.go:") {
		t.Fatalf("detached record stack = %q", stack)
	}
}

// Cluster-scoped objects log no namespace, and objects whose TypeMeta was
// never set (the usual case for typed clients) log no apiVersion or kind.
func TestObjectIdentityOmitsEmptyFields(t *testing.T) {
	var output bytes.Buffer
	logger, err := parseLogging(t, testJSONFormatFlag).Logger(&output)
	if err != nil {
		t.Fatal(err)
	}
	logger.Info(testLogMessage, "object", &corev1.Namespace{ObjectMeta: metav1.ObjectMeta{Name: "tenant"}})
	object, ok := decodeLog(t, output.Bytes())["object"].(map[string]any)
	if !ok || len(object) != 1 || object["name"] != "tenant" {
		t.Fatalf("object identity = %v", object)
	}
}

// ErrorLog also accepts loggers built outside this package.
func TestErrorLogForeignHandler(t *testing.T) {
	var output bytes.Buffer
	ErrorLog(slog.New(slog.NewJSONHandler(&output, nil))).Print("http: test diagnostic")
	record := decodeLog(t, output.Bytes())
	if record["level"] != "ERROR" || record["msg"] != "http: test diagnostic" {
		t.Fatalf("ErrorLog record = %v", record)
	}
}
