//! Manual smoke for the rig LLM client.
//! Usage:
//!   cargo run -p meeting-companion-server --example llm_smoke -- "your description"

use meeting_companion_server::llm::LlmClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let description = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Q1 budget review for helix product launch".to_string());

    tracing_subscriber::fmt::init();

    println!("Initializing LLM client...");
    let client = LlmClient::from_env().await?;
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
