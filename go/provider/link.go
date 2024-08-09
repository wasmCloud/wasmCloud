package provider

// NOTE(brooksmtownsend): There might be a better way to represent this in Go, please comment
// or leave an issue if you can think of one. Perhaps I could do the decryption during the
// unmarshalling process, but I'm not sure if that would be a good idea.
type linkWithEncryptedSecrets struct {
	SourceID      string            `json:"source_id,omitempty"`
	Target        string            `json:"target,omitempty"`
	Name          string            `json:"name,omitempty"`
	WitNamespace  string            `json:"wit_namespace,omitempty"`
	WitPackage    string            `json:"wit_package,omitempty"`
	Interfaces    []string          `json:"interfaces,omitempty"`
	SourceConfig  map[string]string `json:"source_config,omitempty"`
	TargetConfig  map[string]string `json:"target_config,omitempty"`
	SourceSecrets *[]byte           `json:"source_secrets,omitempty"`
	TargetSecrets *[]byte           `json:"target_secrets,omitempty"`
	// Serialized & encrypted secrets. Should decrypt + deserialize into map[string]SecretValue
}

type InterfaceLinkDefinition struct {
	SourceID      string                 `json:"source_id,omitempty"`
	Target        string                 `json:"target,omitempty"`
	Name          string                 `json:"name,omitempty"`
	WitNamespace  string                 `json:"wit_namespace,omitempty"`
	WitPackage    string                 `json:"wit_package,omitempty"`
	Interfaces    []string               `json:"interfaces,omitempty"`
	SourceConfig  map[string]string      `json:"source_config,omitempty"`
	TargetConfig  map[string]string      `json:"target_config,omitempty"`
	SourceSecrets map[string]SecretValue `json:"source_secrets,omitempty"`
	TargetSecrets map[string]SecretValue `json:"target_secrets,omitempty"`
}
