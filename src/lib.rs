//! Rust SDK for [Replane](https://replane.dev): dynamic configuration with real-time updates.
//!
//! The client keeps an SSE connection to the Replane server, holds every config
//! of the SDK key's project and environment in memory, and evaluates overrides
//! locally, so reads are synchronous and the context never leaves your process.
//!
//! ```no_run
//! use replane::{ConnectOptions, Context, Replane};
//!
//! #[tokio::main]
//! async fn main() -> replane::Result<()> {
//!     let replane = Replane::builder()
//!         .default_value("rate-limit", 100)
//!         .connect(ConnectOptions::new("https://replane.example.com", "rp_..."))
//!         .await?;
//!
//!     let ctx = Context::new().with("plan", "pro");
//!     let limit: u32 = replane.get_with("rate-limit", &ctx)?;
//!     println!("rate limit: {limit}");
//!     Ok(())
//! }
//! ```

mod client;
mod context;
mod error;
mod evaluation;
mod js;
mod sse;
mod types;

pub use client::{ConnectOptions, Replane, ReplaneBuilder, Subscription, REPLANE_CLIENT_ID_KEY};
pub use context::{Context, ContextValue};
pub use error::{ReplaneError, Result};
pub use evaluation::{evaluate_condition, evaluate_overrides, EvaluationResult};
pub use types::{Condition, Config, ConfigChange, Override, Snapshot};
