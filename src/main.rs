mod domain;
mod migration;
use serde_json::Value;
use tezbar_extension_sdk::{BoxFuture, CoreResult, Extension, ExtensionContext};
struct Adapter;
impl Extension for Adapter {
    fn id(&self) -> &'static str {
        "com.tezbar.native.amphetamine"
    }
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
    fn commands(&self) -> &'static [&'static str] {
        domain::commands()
    }
    fn initialize<'a>(&'a self, ctx: &'a ExtensionContext) -> BoxFuture<'a, CoreResult> {
        Box::pin(async move {
            migration::initialize(ctx, "amphetamine").await?;
            domain::initialize(ctx)?;
            Ok(serde_json::json!({"ready":true}))
        })
    }
    fn handle<'a>(
        &'a self,
        ctx: &'a ExtensionContext,
        command: &'a str,
        operation: &'a str,
        payload: &'a Value,
        state: &'a mut Value,
    ) -> BoxFuture<'a, CoreResult> {
        Box::pin(domain::handle(ctx, command, operation, payload, state))
    }
    fn shutdown<'a>(&'a self, ctx: &'a ExtensionContext) -> BoxFuture<'a, CoreResult> {
        Box::pin(async move {
            let _ = ctx;
            domain::shutdown_extension(ctx).await
        })
    }
}
fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("extension runtime");
    let result = runtime.block_on(tezbar_extension_sdk::serve(Adapter));
    // Domain/task/protocol cleanup completes before the bounded runtime teardown.
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
