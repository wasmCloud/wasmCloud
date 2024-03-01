package wasmcloud.access

default allow = false

# This policy service isn't concerned with invocations, so allow them immediately
allow {
    input.kind == "performInvocation"
}

# Rule to allow access if conditions are met for startComponent
allow {
    input.kind == "startComponent"
    has_claims
    issued_by_wasmcloud
}

# Rule to allow access if conditions are met for startProvider
allow {
    input.kind == "startProvider"
    has_claims
    issued_by_wasmcloud
}

# Check if the claims object exists (ensure the component is signed)
has_claims {
    input.request.claims != null
}

# Check if the issuer is wasmCloud's official issuer
issued_by_wasmcloud {
    input.request.claims.issuer == "ACOJJN6WUP4ODD75XEBKKTCCUJJCY5ZKQ56XVKYK4BEJWGVAOOQHZMCW"
}