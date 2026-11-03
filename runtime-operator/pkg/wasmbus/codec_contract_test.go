package wasmbus

import (
	"reflect"
	"strings"
	"testing"
)

const (
	codecJSONFixture = `{"name":"test"}`
	unsupportedMIME  = "application/xml"
)

func TestEncodeMimetypeWireFormats(t *testing.T) {
	for _, tc := range []struct {
		mime string
		wire string
	}{
		{mimetypeJSON, codecJSONFixture},
		{"", codecJSONFixture},
		{mimeTypeYAML, "name: test\n"},
	} {
		t.Run(tc.mime, func(t *testing.T) {
			data, err := EncodeMimetype(struct {
				Name string `json:"name" yaml:"name"`
			}{Name: testName}, tc.mime)
			if err != nil || string(data) != tc.wire {
				t.Fatalf("EncodeMimetype = %q, %v; want %q", data, err, tc.wire)
			}
		})
	}
	if data, err := EncodeMimetype(testMessage{Name: testName}, unsupportedMIME); err == nil || data != nil {
		t.Fatalf("unsupported format = %q, %v; want no encoded data and an error", data, err)
	}
}

func TestDecodeEmptyBodyPreservesDefaults(t *testing.T) {
	for _, contentType := range []string{"", mimetypeJSON, mimeTypeYAML, unsupportedMIME} {
		t.Run(contentType, func(t *testing.T) {
			msg := NewMessage(testSubject)
			msg.Header.Set("Content-Type", contentType)
			response := testMessage{Name: testName, NotDecoded: "default"}
			before := response
			if err := Decode(msg, &response); err != nil || response != before {
				t.Fatalf("empty response = %+v, %v; want %+v", response, err, before)
			}
		})
	}
}

func TestMessageSubjectParts(t *testing.T) {
	for _, tc := range []struct {
		subject string
		parts   []string
	}{
		{"runtime.host.heartbeat", []string{"runtime", "host", "heartbeat"}},
		{"single", []string{"single"}},
		{"", []string{""}},
		{"runtime.", []string{"runtime", ""}},
	} {
		t.Run(tc.subject, func(t *testing.T) {
			msg := NewMessage(tc.subject)
			if got := msg.SubjectParts(); !reflect.DeepEqual(got, tc.parts) {
				t.Fatalf("SubjectParts = %q, want %q", got, tc.parts)
			}
			if got := msg.LastSubjectPart(); got != tc.parts[len(tc.parts)-1] {
				t.Fatalf("LastSubjectPart = %q, want %q", got, tc.parts[len(tc.parts)-1])
			}
		})
	}
}

func TestNewInboxIsUniqueLiteralSubject(t *testing.T) {
	first, second := NewInbox(), NewInbox()
	if first == second {
		t.Fatalf("duplicate reply inbox %q", first)
	}
	for _, inbox := range []string{first, second} {
		if !strings.HasPrefix(inbox, "_INBOX.") || strings.ContainsAny(inbox, "*> \t\r\n") {
			t.Fatalf("invalid reply inbox %q", inbox)
		}
	}
}
