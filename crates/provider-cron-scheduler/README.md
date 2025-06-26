# Cron Job Capability Provider

This capability provider enables distributed, fault-tolerant scheduled execution of tasks using cron expressions within the wasmCloud ecosystem. It uses the [wasmcloud-provider-sdk](https://crates.io/crates/wasmcloud-provider-sdk) and implements the [Cron Scheduler](https://docs.rs/wasmcloud-provider-sdk/0.5.0/wasmcloud_provider_sdk/trait.Provider.html) trait to manage scheduled tasks based on cron expressions.

## Features

- **Distributed scheduling** with coordination across multiple provider instances
- **Exactly-once execution** of jobs using NATS-based distributed locking
- **Fault tolerance** with automatic failover if an instance goes down
- **Support for complex cron expressions** including fixed intervals and dynamic scheduling patterns
- **Dynamic payload delivery** to target components
- **Real-time configuration** through link definitions

## Requirements

- **NATS server v2.11.2 or higher** with JetStream enabled
- [Rust toolchain](https://www.rust-lang.org/tools/install)
- [wash](https://wasmcloud.com/docs/installation)

## How It Works

This provider uses NATS JetStream for distributed coordination of cron jobs:

1. Each cron job has a dedicated NATS stream with configured TTL (Time To Live)
2. When a job's message expires, a delete marker triggers execution
3. A distributed locking mechanism ensures only one provider instance executes the job
4. The system automatically adapts to different cron patterns (fixed intervals vs. dynamic schedules)

# Building your application

## Component Interface

To receive scheduled invocations, your component must implement the `wasmcloud:cron` interface:

```wit
// WIT interface definition
package wasmcloud:cron@0.1.0;

interface scheduler {
    invoke: func(payload: list<u8>) -> result<_, string>;
}
```

This interface defines a single function, `invoke`, which receives a binary payload containing the data you specified in your cron job configuration.

## Implementation Example

Here's how to implement the cron interface in your component:

```rust
wit_bindgen::generate!({ generate_all });

use crate::exports::wasmcloud::cron::scheduler::Guest as CronDemoGuest;
use serde_json::Value;
use wasmcloud_component::wasi::logging::logging::{log, Level};

#[derive(Debug)]
struct CronDemo {}

impl CronDemoGuest for CronDemo {
    fn invoke(payload: Vec<u8>) -> Result<(), String> {
        // Unmarshall the byte payload into JSON
        let json_result: Result<Value, _> = serde_json::from_slice(&payload);

        match json_result {
            Ok(json_data) => {
                log(
                    Level::Info,
                    "cron-handler",
                    &format!("Received scheduled task with payload: {}", json_data),
                );
                
                // Process your scheduled task here
                // The payload contains whatever JSON you specified in the link definition
                
                Ok(())
            }
            Err(e) => {
                log(
                    Level::Error,
                    "cron-handler",
                    &format!("Failed to parse payload as JSON: {}", e),
                );
                Err(format!("JSON parsing error: {}", e))
            }
        }
    }
}

export!(CronDemo);
```

In this example:
1. The component implements the `wasmcloud:cron` interface
2. When a scheduled job triggers, the `invoke` function receives the payload bytes
3. The code unmarshalls the JSON payload you specified in your link configuration
4. You can then process the scheduled task as needed

# Configuring your deployment

## Using the Bundled NATS Server
Instead of setting up a standalone NATS server, you can leverage the NATS server that's already included with your wasmCloud host. This bundled NATS:

- Is fully compatible with the Cron Job Capability Provider
- Has JetStream enabled by default (required for the provider's distributed coordination)
- Uses the same lattice infrastructure that wasmCloud components connect to
- Eliminates the need for separate NATS configuration

When using wash up to start wasmCloud, the bundled NATS server is automatically started and configured appropriately.

### Simplified Setup
With the bundled NATS, your workflow becomes simpler:

- Start wasmCloud using wash up (or wash up -d for background mode)
- Configure your deployment including the providers and your components in `wadm.yml` then create links between the two with the appropriate link config. as follows 👇

```yaml
    - name: cronProvider
      type: capability
      properties:
        # Placeholder
        image: ghcr.io/wasmcloud/cron-scheduler:0.1.0
      traits:
        - type: spreadscaler
          properties:
            instances: 1
        # Link the provider to component      
        - type: link
          properties:
            target: yourComponentName # Replace with the name of you component
            namespace: wasmcloud
            package: cron
            interfaces: [scheduler]
            target_config:
              - name: cron-config
                properties:
                  cluster_uri: nats://127.0.0.1:4222
                  cronjobs: job_1=0 * 1 * * ?:{"task":"daily_report"};
```

The `cronjobs` property accepts a semicolon-separated list of cron jobs in the format:
```
job_name=cron_expression:json_payload;
```

For example:
```
daily_report=0 0 0 * * *:{"type":"generate_report"};hourly_update=0 0 * * * *:{"action":"refresh_data"};every_5min=0 */5 * * * *{"task":"check_status"}
```

Each job is defined on its own line within the `cronjobs` value.

### Cron Expression Format and Types

The provider supports standard cron expressions with six fields (including seconds):

```
┌───────────── second (0-59)
│ ┌───────────── minute (0-59)
│ │ ┌───────────── hour (0-23)
│ │ │ ┌───────────── day of the month (1-31)
│ │ │ │ ┌───────────── month (1-12)
│ │ │ │ │ ┌───────────── day of the week (0-6) (Sunday to Saturday)
│ │ │ │ │ │
│ │ │ │ │ │
* * * * * *
```

The provider intelligently analyzes your cron expressions and optimizes job scheduling based on the pattern type:

### Fixed Interval Expressions

These are patterns that occur at regular intervals:

- `*/3 * * * * ?`: Every 3 seconds
- `0 */5 * * * *`: Every 5 minutes
- `0 0 */2 * * *`: Every 2 hours
- `0 0 0 * * *`: Daily at midnight

### Complex/Dynamic Interval Expressions

These are more complex patterns with irregular schedules:

- `0 0 9,12,15 * * *`: At 9am, 12pm, and 3pm every day
- `0 0 0 1,15 * *`: At midnight on the 1st and 15th of each month
- `0 0 9-17 * * 1-5`: Every hour from 9am to 5pm on weekdays

The provider automatically detects the expression type and uses the most efficient scheduling mechanism for each job type.
### Distributed Operation

This provider is designed for distributed operation with the following characteristics:

- Multiple instances can run simultaneously for high availability
- Distributed locking ensures jobs execute exactly once
- Automatic failover if a provider instance fails
- Job execution is coordinated across all instances using NATS JetStream
- Different job types (fixed interval vs dynamic scheduling) are handled efficiently

## Running as an Application

Deploy the provider along with a test component:

```bash
# Launch wasmCloud in the background
wash up -d
# Deploy the application
wash app deploy ./wadm.yaml
```

Have questions? Please [file an issue](https://github.com/wasmCloud/wasmCloud/issues/new/choose) or join us on the [wasmCloud slack](https://slack.wasmcloud.com)!