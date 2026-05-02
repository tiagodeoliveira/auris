//! Mock content generator. Produces fake items for Phase 0 so the PWA
//! has something to render. See `docs/specs/server.md` §8.6.

use crate::contract::Item;
use std::time::Instant;
use uuid::Uuid;

pub const HIGHLIGHTS: &[&str] = &[
    "Tiago raised concern about Q1 budget overrun",
    "Decision: ship feature X by end of sprint",
    "Open question: who owns the migration",
    "Action item: schedule follow-up with vendor",
    "Aline highlighted the dependency on the auth team",
    "Push the launch date by two weeks",
    "Concern: test coverage gap in the new module",
    "Confirmed: customer is OK with the proposed timeline",
];

pub const TRANSCRIPT: &[&str] = &[
    "Speaker A: I think we should delay the launch by two weeks.",
    "Speaker B: Acknowledged. Let me check with engineering.",
    "Speaker A: The dependency on the auth team is the blocker.",
    "Speaker C: I can take the auth conversation offline.",
    "Speaker A: Great. What about the migration plan?",
    "Speaker B: Draft is ready, sending tonight.",
    "Speaker C: Are we testing against staging first?",
    "Speaker A: Yes, full staging soak before prod.",
    "Speaker B: Agreed. We'll set up the soak window.",
    "Speaker A: Anything else? OK, ending here.",
];

pub const ACTIONS: &[&str] = &[
    "Tiago: Draft proposal by Friday",
    "Aline: Confirm vendor availability",
    "Speaker C: Sync with auth team on dependency",
    "Speaker B: Send migration draft tonight",
    "Speaker A: Schedule staging soak window",
    "Tiago: Update launch date in roadmap",
];

pub fn template_for(mode_id: &str) -> &'static [&'static str] {
    match mode_id {
        "highlights" => HIGHLIGHTS,
        "transcript" => TRANSCRIPT,
        "actions" => ACTIONS,
        _ => HIGHLIGHTS,
    }
}

pub fn make_item(mode_id: &str, tick_index: usize, started_at: Instant) -> Item {
    let templates = template_for(mode_id);
    let text = templates[tick_index % templates.len()].to_string();
    let t_ms = started_at.elapsed().as_millis() as u64;
    Item {
        id: Uuid::new_v4().to_string(),
        text,
        detail: None,
        t: t_ms,
        meta: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_item_uses_correct_template() {
        let started = Instant::now();
        let i0 = make_item("highlights", 0, started);
        assert_eq!(i0.text, HIGHLIGHTS[0]);
        let i1 = make_item("transcript", 1, started);
        assert_eq!(i1.text, TRANSCRIPT[1]);
    }

    #[test]
    fn make_item_wraps_around() {
        let started = Instant::now();
        let i = make_item("highlights", HIGHLIGHTS.len(), started);
        assert_eq!(i.text, HIGHLIGHTS[0]);
    }

    #[test]
    fn make_item_unique_ids() {
        let started = Instant::now();
        let a = make_item("actions", 0, started);
        let b = make_item("actions", 0, started);
        assert_ne!(a.id, b.id);
    }
}
