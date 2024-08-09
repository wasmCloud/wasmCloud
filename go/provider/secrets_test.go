package provider

import (
	"encoding/json"
	"testing"
)

func TestUnmarshalJson(t *testing.T) {
	// Define the JSON input
	jsonData := `{"kind": "String", "value": "mySecretValue"}`

	// Create a new SecretValue instance
	secret := &SecretValue{}

	// Unmarshal the JSON data into the SecretValue instance
	err := json.Unmarshal([]byte(jsonData), secret)
	if err != nil {
		t.Errorf("Failed to unmarshal JSON: %v", err)
	}

	// Verify the unmarshaled value
	expectedValue := "mySecretValue"
	if secret.String.Reveal() != expectedValue {
		t.Errorf("Unexpected value. Got: %s, Expected: %s", secret.String.Reveal(), expectedValue)
	}
}

func TestUnmarshalJsonMap(t *testing.T) {
	// Define the JSON input
	jsonData := `{"foobar": {"kind": "String", "value": "mySecretValue"}}`

	// Create a new SecretValue instance
	secret := make(map[string]SecretValue)

	// Unmarshal the JSON data into the SecretValue instance
	err := json.Unmarshal([]byte(jsonData), &secret)
	if err != nil {
		t.Errorf("Failed to unmarshal JSON: %v", err)
	}

	// Verify the unmarshaled value
	expectedValue := "mySecretValue"
	if secret["foobar"].String.Reveal() != expectedValue {
		t.Errorf("Unexpected value. Got: %s, Expected: %s", secret["foobar"].String.Reveal(), expectedValue)
	}
}
