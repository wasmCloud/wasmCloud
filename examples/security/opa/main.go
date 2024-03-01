package main

import (
	"context"
	"encoding/json"
	"fmt"
	"log"

	"github.com/google/uuid"
	"github.com/nats-io/nats.go"
	"github.com/open-policy-agent/opa/rego"
)

func evaluatePolicy(m *nats.Msg) {
	ctx := context.Background()

	// Load the policy from disk
	query, err := rego.New(
		rego.Query("data.wasmcloud.access.allow"),
		rego.Load([]string{"./policy.rego"}, nil),
	).PrepareForEval(ctx)
	if err != nil {
		log.Fatalf("Failed to prepare for eval: %s", err)
	}

	// Convert the JSON input string to a map for the OPA evaluation
	var inputMap map[string]interface{}
	err = json.Unmarshal(m.Data, &inputMap)
	if err != nil {
		log.Fatalf("Failed to unmarshal input: %s", err)
	}

	// Generate a UUID for the requestId
	requestId := uuid.New().String()

	// Evaluate the policy
	results, err := query.Eval(ctx, rego.EvalInput(inputMap))
	if err != nil {
		log.Fatalf("Failed to evaluate policy: %s", err)
	}

	// Prepare the response based on the evaluation result
	response := map[string]interface{}{
		"requestId": requestId,
	}
	if len(results) > 0 && results[0].Expressions[0].Value == true {
		fmt.Println("Listening for policy requests...")
		response["permitted"] = true
	} else {
		response["permitted"] = false
	}

	// Marshal the response into JSON
	responseJSON, err := json.Marshal(response)
	if err != nil {
		log.Fatalf("Failed to marshal response: %s", err)
	}

	// Send the response back via NATS
	if err := m.Respond(responseJSON); err != nil {
		log.Fatalf("Failed to publish response: %s", err)
	}
}

func main() {
	// Connect to NATS server
	nc, err := nats.Connect(nats.DefaultURL)
	if err != nil {
		log.Fatal(err)
	}
	defer nc.Close()

	// Subscribe to the "wasmcloud.policy" subject
	_, err = nc.Subscribe("wasmcloud.policy", evaluatePolicy)

	if err != nil {
		log.Fatal(err)
	}

	fmt.Println("Listening for policy requests...")
	// Keep the connection alive
	select {}
}
