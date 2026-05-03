//! Integration test for the rig-backed LLM client. Requires real AWS
//! credentials and Sonnet 4.7 enabled in Bedrock.
//!
//! Skipped by default. Run with:
//!   RUN_LLM_INTEGRATION=1 cargo test -p meeting-companion-server --test llm_integration

#[tokio::test]
async fn extracts_title_from_real_description() {
    if std::env::var("RUN_LLM_INTEGRATION").is_err() {
        return;
    }
    std::env::remove_var("MEETING_COMPANION_LLM_DISABLED");

    let client = meeting_companion_server::llm::LlmClient::from_env()
        .await
        .expect("LLM client init");

    let result = client
        .extract("Q1 budget review for the helix product launch and rollout plan")
        .await
        .expect("extraction succeeded");

    let title = result.get("title").expect("title key present");
    assert!(!title.is_empty(), "title is empty");
    let word_count = title.split_whitespace().count();
    assert!(
        word_count <= 8,
        "title '{}' has {} words; expected ≤ 8",
        title,
        word_count
    );

    if let Some(project) = result.get("project") {
        assert!(!project.is_empty(), "project key present but empty");
    }
}
