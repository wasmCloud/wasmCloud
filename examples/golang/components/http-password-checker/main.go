//go:generate go tool wit-bindgen-go generate --world hello --out gen ./wit

package main

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"

	gopasswordvalidator "github.com/wagslane/go-password-validator"
	"go.wasmcloud.dev/component/net/wasihttp"
)

type CheckRequest struct {
	Value string `json:"value"`
}

type CheckResponse struct {
	Valid   bool   `json:"valid"`
	Length  int    `json:"length,omitempty"`
	Message string `json:"message,omitempty"`
}

func init() {
	mux := http.NewServeMux()
	mux.HandleFunc("/api/v1/check", handleRequest)
	wasihttp.Handle(mux)
}

func handleRequest(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		errResponseJSON(w, http.StatusMethodNotAllowed, "Method not allowed")
		return
	}

	var req CheckRequest
	defer r.Body.Close()

	b, err := io.ReadAll(r.Body)
	if err != nil {
		errResponseJSON(w, http.StatusBadRequest, err.Error())
		return
	}

	if err := json.Unmarshal(b, &req); err != nil {
		errResponseJSON(w, http.StatusBadRequest, fmt.Sprintf("error with json input: %s", err.Error()))
		return
	}

	err = gopasswordvalidator.Validate(req.Value, 60)
	if err != nil {
		errResponseJSON(w, http.StatusBadRequest, err.Error())
		return
	}

	resp := CheckResponse{Valid: true, Length: len(req.Value)}
	respJSON, err := json.Marshal(resp)
	if err != nil {
		errResponseJSON(w, http.StatusInternalServerError, err.Error())
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.Write(respJSON)
}

func errResponseJSON(w http.ResponseWriter, code int, message string) {
	msg, _ := json.Marshal(CheckResponse{Valid: false, Message: message})
	http.Error(w, string(msg), code)
	w.Header().Set("Content-Type", "application/json")
}

// Since we don't run this program like a CLI, the `main` function is empty. Instead,
// we call the `handleRequest` function when an HTTP request is received.
func main() {}
