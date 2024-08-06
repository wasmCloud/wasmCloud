package provider

const (
	OtelProtocolHTTP = "Http"
	OtelProtocolGRPC = "Grpc"
)

type OtelConfig struct {
	EnableObservability   bool   `json:"enable_observability"`
	EnableTraces          *bool  `json:"enable_traces,omitempty"`
	EnableMetrics         *bool  `json:"enable_metrics,omitempty"`
	EnableLogs            *bool  `json:"enable_logs,omitempty"`
	ObservabilityEndpoint string `json:"observability_endpoint,omitempty"`
	TracesEndpoint        string `json:"traces_endpoint,omitempty"`
	MetricsEndpoint       string `json:"metrics_endpoint,omitempty"`
	LogsEndpoint          string `json:"logs_endpoint,omitempty"`
	TraceLevel            *Level `json:"trace_level,omitempty"`
	Protocol              string `json:"protocol,omitempty"`
}

type HostData struct {
	HostID                 string                     `json:"host_id,omitempty"`
	LatticeRPCPrefix       string                     `json:"lattice_rpc_prefix,omitempty"`
	LatticeRPCUserJWT      string                     `json:"lattice_rpc_user_jwt,omitempty"`
	LatticeRPCUserSeed     string                     `json:"lattice_rpc_user_seed,omitempty"`
	LatticeRPCURL          string                     `json:"lattice_rpc_url,omitempty"`
	ProviderKey            string                     `json:"provider_key,omitempty"`
	EnvValues              map[string]string          `json:"env_values,omitempty"`
	InstanceID             string                     `json:"instance_id,omitempty"`
	LinkDefinitions        []linkWithEncryptedSecrets `json:"link_definitions,omitempty"`
	ClusterIssuers         []string                   `json:"cluster_issuers,omitempty"`
	Config                 map[string]string          `json:"config,omitempty"`
	Secrets                map[string]SecretValue     `json:"secrets,omitempty"`
	HostXKeyPublicKey      string                     `json:"host_xkey_public_key,omitempty"`
	ProviderXKeyPrivateKey SecretStringValue          `json:"provider_xkey_private_key,omitempty"`
	DefaultRPCTimeoutMS    *uint64                    `json:"default_rpc_timeout_ms,omitempty"`
	StructuredLogging      bool                       `json:"structured_logging,omitempty"`
	LogLevel               *Level                     `json:"log_level,omitempty"`
	OtelConfig             OtelConfig                 `json:"otel_config,omitempty"`
}

type HealthCheckResponse struct {
	Healthy bool   `json:"healthy"`
	Message string `json:"message,omitempty"`
}
