# catbus-client

Rust client for the Catbus WebTransport message bus.

## Features

- WebTransport connection via `wtransport`
- Pub/sub messaging with wildcard pattern support
- Correlation ID-based request/response pattern
- Automatic reconnection with exponential backoff
- Keepalive ping/pong

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
catbus-client = { path = "../catbus/client-rs" }
tokio = { version = "1", features = ["full"] }
serde_json = "1.0"
```

### Basic Example

```rust
use catbus_client::{CatbusClient, CatbusConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create client with config
    let config = CatbusConfig::new("https://localhost:4433", "your-auth-token")
        .dangerous_skip_cert_verify(); // For self-signed certs in development

    let client = CatbusClient::new(config);

    // Connect to the server
    client.connect().await?;

    // Subscribe to messages
    let sub = client.subscribe("runner.task.*", |channel, payload| {
        println!("Received on {}: {:?}", channel, payload);
    }).await?;

    // Publish a message
    client.publish("runner.task.123.status", &serde_json::json!({
        "status": "running",
        "progress": 50
    })).await?;

    // Cleanup
    sub.unsubscribe().await?;
    client.disconnect().await?;

    Ok(())
}
```

### Request/Response Pattern

The client supports correlation ID-based request/response:

```rust
use catbus_client::{CatbusClient, CatbusConfig};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct TaskAssignment {
    task_id: String,
}

#[derive(Deserialize)]
struct TaskResult {
    success: bool,
    output: String,
}

async fn assign_task(client: &CatbusClient, task_id: &str) -> Result<TaskResult, catbus_client::CatbusError> {
    // Send request and wait for response
    let result: TaskResult = client.request(
        &format!("runner.task.{}.assign", task_id),
        &TaskAssignment { task_id: task_id.to_string() },
        &format!("runner.task.{}.reply", task_id),
    ).await?;

    Ok(result)
}
```

### Channel Patterns

Catbus supports wildcard subscriptions:

- `runner.task.123` - Exact match
- `runner.task.*` - Matches all channels under `runner.task.`
- `*` - Matches all channels (admin only)

### Runner Channels (from OE-81)

The Plan Runner uses these channels:

| Channel Pattern | Purpose |
|----------------|---------|
| `runner.task.{id}.assign` | Task assignment requests |
| `runner.task.{id}.reply` | Task completion responses |
| `runner.plan.{id}.status` | Plan status updates |
| `runner.agent.{id}.status` | Agent status updates |

## Configuration

```rust
use std::time::Duration;
use catbus_client::CatbusConfig;

let config = CatbusConfig::new("https://localhost:4433", "token")
    .no_reconnect()                                    // Disable auto-reconnect
    .reconnect_delay(Duration::from_secs(1), Duration::from_secs(60))  // Custom backoff
    .ping_interval(Duration::from_secs(15))            // Custom ping interval
    .operation_timeout(Duration::from_secs(30))        // Custom timeout
    .dangerous_skip_cert_verify();                     // Skip TLS verification (dev only)
```

## Connection States

Monitor connection state changes:

```rust
let mut state_rx = client.state_receiver();

tokio::spawn(async move {
    while state_rx.changed().await.is_ok() {
        println!("Connection state: {:?}", *state_rx.borrow());
    }
});
```

States: `Disconnected`, `Connecting`, `Authenticating`, `Connected`, `Reconnecting`

## Integration with Obelus

This client is designed to integrate with the Obelus project's Plan Runner:

```rust
// In obelus-common or runner crate
use catbus_client::{CatbusClient, CatbusConfig};

pub struct RunnerCatbus {
    client: CatbusClient,
}

impl RunnerCatbus {
    pub async fn new(url: &str, token: &str) -> Result<Self, catbus_client::CatbusError> {
        let config = CatbusConfig::new(url, token)
            .dangerous_skip_cert_verify(); // Configure based on environment

        let client = CatbusClient::new(config);
        client.connect().await?;

        Ok(Self { client })
    }

    pub async fn publish_plan_status(&self, plan_id: &str, status: &str) -> Result<(), catbus_client::CatbusError> {
        self.client.publish(
            &format!("runner.plan.{}.status", plan_id),
            &serde_json::json!({ "status": status })
        ).await
    }
}
```
