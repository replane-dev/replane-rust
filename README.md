<h1 align="center">Replane Rust SDK</h1>
<p align="center">Dynamic configuration for Rust applications.</p>

<p align="center">
  <a href="https://crates.io/crates/replane"><img src="https://img.shields.io/crates/v/replane" alt="crates.io"></a>
  <a href="https://docs.rs/replane"><img src="https://img.shields.io/docsrs/replane" alt="docs.rs"></a>
  <a href="https://github.com/replane-dev/replane-rust/actions"><img src="https://github.com/replane-dev/replane-rust/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/replane-dev/replane-rust/blob/main/LICENSE"><img src="https://img.shields.io/github/license/replane-dev/replane-rust" alt="License"></a>
  <a href="https://github.com/orgs/replane-dev/discussions"><img src="https://img.shields.io/badge/discussions-join-blue?logo=github" alt="Community"></a>
</p>

Rust client for [Replane](https://replane.dev): dynamic configuration with real-time updates.

The client keeps a Server-Sent Events connection to Replane, holds all configs for the SDK key's
project and environment in memory, and evaluates overrides locally. Reads are synchronous and the
evaluation context never leaves your process.

## Installation

```toml
[dependencies]
replane = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

TLS uses rustls by default. To use the platform's native TLS instead:

```toml
replane = { version = "0.1", default-features = false, features = ["native-tls"] }
```

## Quick start

```rust
use replane::{ConnectOptions, Context, Replane};

#[tokio::main]
async fn main() -> replane::Result<()> {
    let replane = Replane::builder()
        .default_value("rate-limit", 100)
        .connect(ConnectOptions::new("https://replane.example.com", "rp_..."))
        .await?;

    // Overrides are evaluated against the merged client and per-call context.
    let ctx = Context::new().with("userId", "u-42").with("plan", "pro");
    let limit: u32 = replane.get_with("rate-limit", &ctx)?;
    println!("rate limit: {limit}");
    Ok(())
}
```

`connect` waits until the initial configs arrive (5 seconds by default) and then keeps the
connection open in the background, reconnecting with exponential backoff if it drops.

## Reading configs

Any type implementing `serde::Deserialize` can be read, including your own structs:

```rust
#[derive(serde::Deserialize)]
struct Checkout {
    enabled: bool,
    max_items: u32,
}

let checkout: Checkout = replane.get("checkout")?;
let raw: serde_json::Value = replane.get("checkout")?;
let enabled = replane.get_or("new-ui", false); // default if missing or mistyped
```

## Context

```rust
use replane::{Context, Replane};

// Context applied to every read from this client.
let replane = Replane::builder()
    .context(Context::new().with("region", "eu"))
    .build();

// A cheap clone that shares configs and the connection, with extra context.
let user_client = replane.with_context(&Context::new().with("userId", user.id));
let enabled: bool = user_client.get("new-checkout")?;
```

Context values can be strings, numbers, booleans or `None` (null). Each client also gets a
random `replaneClientId` context value, which can be used for percentage rollouts.

## Subscriptions

```rust
let subscription = replane.subscribe("feature-x", |change| {
    println!("{} is now {}", change.name, change.value);
});
// Dropping `subscription` unsubscribes; call `subscription.detach()` to keep it forever.
```

The callback receives the base value (without overrides) and runs on the connection task, so keep
it short. Call `get` inside it to get the value for a specific context.

## Defaults and snapshots

Defaults make the client usable before (or without) a connection, and are kept for configs the
server doesn't have. A snapshot from `replane.snapshot()` is serializable and can seed another
client:

```rust
let snapshot = replane.snapshot();
let json = serde_json::to_string(&snapshot)?;

let restored = Replane::builder()
    .snapshot(serde_json::from_str(&json)?)
    .build();
```

For tests, build a client with defaults and never connect it:

```rust
let replane = Replane::builder()
    .default_value("feature-enabled", true)
    .default_value("rate-limit", 100)
    .build();
```

## Connection options

| Option               | Default                    | Description                                                |
| -------------------- | -------------------------- | ---------------------------------------------------------- |
| `connect_timeout`    | 5s                         | How long `connect` waits for the initial configs           |
| `request_timeout`    | 2s                         | Timeout for establishing each stream request               |
| `inactivity_timeout` | 30s                        | Reconnect if no events or heartbeats arrive for this long  |
| `retry_delay`        | 200ms                      | Initial reconnect delay, doubled per failure up to 10s     |
| `agent`              | `replane-rust-sdk/<ver>`   | `User-Agent` header                                        |
| `http_client`        | built-in                   | Custom `reqwest::Client` (no total request timeout)        |

## Evaluation semantics

Overrides are evaluated exactly like the Replane server evaluates them for the UI preview: the
first override whose conditions all match wins, a condition on a property missing from the context
never matches, condition values are cast to the context value's type (`"18"` matches `18`), and
percentage segmentation uses FNV-1a over `String(value) + seed`, so users land in the same bucket
as in the JavaScript SDK.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and contribution guidelines.

## Community

Have questions or want to discuss Replane? Join the conversation in [GitHub Discussions](https://github.com/orgs/replane-dev/discussions).

## License

MIT
