//! Manual smoke for the rig LLM client.
//! Usage:
//!   cargo run -p auris-server --example llm_smoke -- "your description"

// LlmClient::extract() nests several async layers; computing this
// example's top-level async block layout overflows rustc's default
// type-recursion depth (same as tests/llm_integration.rs). Raise it
// here rather than constrain the production async shape.
#![recursion_limit = "256"]

use auris_server::llm::LlmClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    let description = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Q1 budget review for helix product launch".to_string());

    tracing_subscriber::fmt::init();

    println!("Initializing LLM client...");
    let client = LlmClient::from_env(auris_server::llm::LlmPool::Background, None).await?;
    println!("Provider: {:?}", client.provider());

    println!("Description: {description}");
    println!("Extracting...");

    let start = std::time::Instant::now();
    let result = client.extract(&description).await?;
    let elapsed = start.elapsed();

    println!("\nResult ({:?}):", elapsed);
    for (k, v) in &result {
        println!("  {k} = {v}");
    }

    Ok(())
}
