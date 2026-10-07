//! Connects to Replane and prints a config whenever it changes.
//!
//! ```sh
//! REPLANE_BASE_URL=http://localhost:8080 REPLANE_SDK_KEY=rp_... REPLANE_CONFIG=my-config \
//!     cargo run --example basic
//! ```

use replane::{ConnectOptions, Context, Replane};
use serde_json::Value;

#[tokio::main]
async fn main() -> replane::Result<()> {
    let base_url =
        std::env::var("REPLANE_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".into());
    let sdk_key = std::env::var("REPLANE_SDK_KEY").expect("set REPLANE_SDK_KEY");
    let config = std::env::var("REPLANE_CONFIG").unwrap_or_else(|_| "my-config".into());

    let replane = Replane::builder()
        .context(Context::new().with("environment", "local"))
        .connect(ConnectOptions::new(base_url, sdk_key))
        .await?;

    println!(
        "{config} = {}",
        replane.get_or::<Value>(&config, Value::Null)
    );

    let _subscription = replane.subscribe(&config, |change| {
        println!("{} changed to {}", change.name, change.value);
    });

    tokio::signal::ctrl_c().await.ok();
    Ok(())
}
