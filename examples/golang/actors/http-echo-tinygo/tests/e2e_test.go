// go:build e2e
//go:build e2e
// +build e2e

package tests

import (
	"bytes"
	"fmt"
	"io/ioutil"
	"net/http"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"runtime"
	"testing"
	"text/template"
	"time"
)

// Template to be used for wadm in tests (see fixtures/wadm.template.yaml)
type WadmTemplate struct {
	AppName           string
	AppVersion        string
	ActorImage        string
	HttpServerPort    int
	HttpServerHost string
}

// Define a function that waits for a predicate to be true
type predicate func() bool

// Wait for a given iven time
func waitForTimeout(t *testing.T, description string, timeout time.Duration, op predicate) {
	ticker := time.NewTicker(500 * time.Millisecond) // Repeated operation every 500ms
	defer ticker.Stop()

	done := make(chan bool, 1)
	go func() {
		for {
			select {
			case <-ticker.C:
				if op() {
					done <- true
				}
			case <-done:
				return
			}
		}
	}()

	select {
	case <-time.After(timeout):
		t.Fatalf("timeout elapsed waiting for [%s]", description)
	case <-done:
		return
	}
}

// Ensure that this can be built
func TestBuild(t *testing.T) {
	// Get the absolute path of the working directory
	_, filename, _, _ := runtime.Caller(0)
	workingDir := path.Join(path.Dir(filename), "..")
	workingDirAbs, err := filepath.Abs(workingDir)
	if err != nil {
		t.Fatalf("failed to get absolute path of working directory: %v\n", err)
	}
	// Find the wash binary
	washBin, err := exec.LookPath("wash")
	if err != nil {
		t.Fatalf("wash binary not found on your path: %v", err)
	}

	// Run the command
	cmd := exec.Command(washBin, "build")
	cmd.Dir = workingDirAbs
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		t.Fatalf("failed to run command: %v\n", err)
	}

	// Start a wasmcloud instance
	cmd = exec.Command(washBin, "up", "--detached")
	cmd.Dir = workingDirAbs
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		t.Fatalf("failed to start detached wash host: %v\n", err)
	}

	// Wait for wasmCloud to start up
	time.Sleep(5 * time.Second)

	// Derive the path to the built actor with the file scheme
	actorImage := fmt.Sprintf("file://%s", path.Join(workingDirAbs, "build", "http-echo-tinygo-component_s.wasm"))

	// Create a temp file to hold wadm configuration for this test
	tempWadmYaml, err := ioutil.TempFile("", "http-echo-tinygo-test-wadm-*.yaml")
	if err != nil {
		t.Fatalf("Failed to create temp file: %v\n", err)
	}
	defer tempWadmYaml.Close()

	// Read the WADM YAML template content
	wadmTemplatePath := path.Join(workingDirAbs, "tests", "fixtures", "wadm.template.yaml")
	tmpl, err := template.New("wadm.template.yaml").ParseFiles(wadmTemplatePath)
	if err != nil {
		t.Fatalf("Failed to create template: %v\n", err)
	}

	// Render the WADM YAML template
	var renderedTemplate bytes.Buffer
	templateArgs := WadmTemplate{
		AppName:           "test-http-echo-tinygo-component",
		AppVersion:        "v0.0.1",
		ActorImage:        actorImage,
		HttpServerPort:    8081,
		HttpServerHost: "127.0.0.1",
	}
	err = tmpl.Execute(&renderedTemplate, templateArgs)
	if err != nil {
		t.Fatalf("Failed to render template: %v\n", err)
	}

	// Write out the temp file
	bytesWritten, err := tempWadmYaml.Write(renderedTemplate.Bytes())
	if bytesWritten != renderedTemplate.Len() || err != nil {
		t.Fatalf("Failed to write bytes to temp yaml file: %v\n", err)
	}

	// Delete existing manifest if already deployed
	cmd = exec.Command(washBin, "app", "delete", "--output", "json", templateArgs.AppName, templateArgs.AppVersion)
	cmd.Dir = workingDirAbs
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		fmt.Printf("[warn] failed to delete existing manifest: %v", err)
		fmt.Printf("(this is fine, if the test has run in the past)")
	}

	// Run wadm to deploy the application
	cmd = exec.Command(washBin, "app", "deploy", tempWadmYaml.Name(), "--output", "json")
	cmd.Dir = workingDirAbs
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		t.Fatalf("failed to deploy with wadm: %v", err)
	}

	// Wait until we can reach the echo actor
	echoActorUrl := fmt.Sprintf("http://%s:%v", templateArgs.HttpServerHost, templateArgs.HttpServerPort)
	// This can take a while because providers must be downloaded
	waitForTimeout(t, fmt.Sprintf("reached echo actor via HTTP @ %v", echoActorUrl), 3*time.Minute, func() bool {
		resp, err := http.Get(echoActorUrl)
		if err != nil {
			return false
		}
		defer resp.Body.Close()
		return true
	})

	// Perform a request against the echo actor, now that we know it's up
	resp, err := http.Get(echoActorUrl)
	if err != nil {
		t.Fatalf("http request failed against echo actor @ [%s]: %v", echoActorUrl, err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("response failed w/ status [%v]", resp.StatusCode)
	}
	defer resp.Body.Close()

	// Stop all hosts
	cmd = exec.Command(washBin, "down", "--all")
	cmd.Dir = workingDirAbs
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		t.Fatalf("failed to deploy with wadm: %v", err)
	}
}
