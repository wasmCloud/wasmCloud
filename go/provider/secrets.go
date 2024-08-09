package provider

import (
	"encoding/json"
	"fmt"

	"github.com/nats-io/nkeys"
)

// Type alias to use for sensitive values to avoid accidentally logging them

type SecretStringValue struct {
	value string
}

func (s SecretStringValue) String() string {
	return "redacted(string)"
}

func (s SecretStringValue) Reveal() string {
	return s.value
}

type SecretBytesValue struct {
	value []byte
}

func (s SecretBytesValue) String() string {
	return "redacted(bytes)"
}

func (s SecretBytesValue) Reveal() []byte {
	return s.value
}

type SecretValue struct {
	String SecretStringValue
	Bytes  SecretBytesValue
}

// Secret values are serialized as either a String or Bytes value, e.g.
// {"kind": "String", "value": "my secret"} or {"kind": "Bytes", "value": [1, 2, 3]}
func (s *SecretValue) UnmarshalJSON(data []byte) error {
	var jsonSecret map[string]interface{}
	err := json.Unmarshal(data, &jsonSecret)
	if err != nil {
		return err
	}

	switch jsonSecret["kind"] {
	case "String":
		s.String = SecretStringValue{value: jsonSecret["value"].(string)}
	case "Bytes":
		s.Bytes = SecretBytesValue{value: jsonSecret["value"].([]byte)}
	default:
		return fmt.Errorf("invalid secret kind: %s", jsonSecret["kind"])
	}

	return nil
}

func (s *SecretStringValue) UnmarshalJSON(data []byte) error {
	var stringValue string
	err := json.Unmarshal(data, &stringValue)
	if err != nil {
		return err
	}
	s.value = stringValue
	return nil
}

func DecryptSecrets(encryptedBytes *[]byte, xkey nkeys.KeyPair, sender string) (map[string]SecretValue, error) {
	var sourceSecrets = make(map[string]SecretValue)
	// If the source secrets are empty or not present, we don't need to decrypt/unmarshal them
	if encryptedBytes != nil && len(*encryptedBytes) >= 0 {
		sourceSecretBytes, err := xkey.Open(*encryptedBytes, sender)
		if err != nil {
			return sourceSecrets, err
		}
		err = json.Unmarshal(sourceSecretBytes, &sourceSecrets)
		if err != nil {
			return sourceSecrets, err
		}
	}
	return sourceSecrets, nil
}
