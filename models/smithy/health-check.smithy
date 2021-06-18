namespace org.wasmcloud.health

/// Return value from actors and providers for health check status
structure HealthCheckResponse {

  /// A flag that indicates the the actor is healthy
  healthy: Bool

  /// A message containing additional information about the actors health
  message: String
}

/// health check request parameter
structure HealthCheckRequest { }

/// Perform health check. Called at regular intervals by host
operation HealthRequest {
    input: HealthCheckRequest
    output: HealthCheckResponse
}
