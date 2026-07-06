//! `recall_meeting` tool — on-demand recall of attached past meetings.

use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AgentToolError, ToolCtx};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct FetchMeetingArgs {
    /// The meeting id of an attached past meeting (from the
    /// "# Attached meetings" block in the working context).
    pub(crate) id: String,
}

/// `recall_meeting` — scoped recall against an attached past meeting.
/// Queries mnemo's `/recall` filtered by `attributes.meeting_id`,
/// returning the per-meeting rollup (actions, highlights, open
/// questions, moment summaries) plus any transcript-derived memories.
/// Replaces the old `fetch_meeting_summary` + `fetch_meeting` pair
/// — those were functionally identical (mnemo treats both as the
/// same scoped recall today).
pub(crate) struct RecallMeeting(pub(crate) ToolCtx);

impl Tool for RecallMeeting {
    const NAME: &'static str = "recall_meeting";
    type Args = FetchMeetingArgs;
    type Output = String;
    type Error = AgentToolError;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::agent::prompts::TOOL_DESC_RECALL_MEETING.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Meeting id from the [attached meetings] list." }
                },
                "required": ["id"]
            }),
        }
    }

    async fn call(&self, args: FetchMeetingArgs) -> Result<String, AgentToolError> {
        let params = crate::mnemo::recall::RecallParams::for_meeting_id(args.id.clone());
        tracing::info!(meeting_id = %args.id, "agent recall_meeting: recalling from mnemo");
        let recalled = self
            .0
            .mnemo
            .recall(&self.0.user_id, &params)
            .await
            .map_err(|e| AgentToolError::Internal(format!("mnemo recall: {e}")))?;
        let s = recalled.summary();
        tracing::info!(
            meeting_id = %args.id,
            preferences = s.preferences,
            facts = s.facts,
            episodes = s.episodes,
            project_memories = s.project_memories,
            "agent recall_meeting: recall complete"
        );
        let body = recalled.format_for_prompt();
        if body.trim().is_empty() {
            Ok(format!(
                "No mnemo memories found for meeting {} (it may have ended before mnemo was \
                 enabled, or the attached id is unknown).",
                args.id
            ))
        } else {
            Ok(format!("Recall for meeting {}:\n\n{body}", args.id))
        }
    }
}
