//! Intent dispatch for `UserSession`.
//!
//! A separate `impl UserSession` block — Rust allows splitting impls across
//! files within the same crate. Methods here call accessors and mutators
//! defined in `session/user.rs`.

use super::{
    AssistSensitivityPersist, ClosedMeetingRecord, IntentOutcome, MeetingRuntime, MomentRequest,
    NewMeetingRecord, WrapUpRequest, DEFAULT_MODE_ID,
};
use crate::protocol::{AssistSensitivity, Event, Intent, MeetingState, Status};
use crate::session::UserSession;
use std::collections::HashMap;

impl UserSession {
    pub fn apply_intent(&mut self, intent: Intent) -> IntentOutcome {
        let mut outcome = IntentOutcome::default();
        match intent {
            Intent::StartMeeting {
                description,
                metadata,
                audio_source_device_id,
                assist_sensitivity,
            } => {
                self.handle_start_meeting(
                    description,
                    metadata,
                    audio_source_device_id,
                    assist_sensitivity.unwrap_or_default(),
                    &mut outcome,
                );
            }
            Intent::StopMeeting => {
                self.handle_stop_meeting(&mut outcome);
            }
            Intent::SetAssistSensitivity { value } => {
                self.handle_set_assist_sensitivity(value, &mut outcome);
            }
            Intent::Pause | Intent::Resume => {
                // Legacy no-op. See contract.rs's Intent enum comment:
                // pause was removed in favor of client-local mute; old
                // TestFlight builds may still send these. Log and drop
                // — no event emitted, no state change.
                tracing::debug!(
                    state = ?self.meeting_state,
                    "legacy pause/resume intent ignored"
                );
            }
            Intent::SetMode { mode } => {
                // Legacy no-op. `currentMode` is now per-surface UI
                // state — each surface (PWA browser, glasses, Mac,
                // mobile) tracks which mode IT is viewing locally
                // and doesn't follow cross-surface broadcasts. The
                // intent is vestigial; we keep the variant + the
                // known-intents allowlist entry so old clients
                // don't see `unknown_intent` toasts during the
                // rollout window. Drop both once deployed clients
                // are confirmed to have stopped sending it.
                tracing::debug!(
                    mode,
                    "legacy set_mode intent ignored — currentMode is per-surface UI state"
                );
            }
            Intent::SetMetadata { key, value } => {
                self.handle_set_metadata(key, value, &mut outcome)
            }
            Intent::MarkMoment {
                t,
                note,
                id,
                self_capture,
            } => self.handle_mark_moment(t, note, id, self_capture, &mut outcome),
            Intent::RegisterDevice { .. } => {
                // Handled in ws.rs because it needs the per-connection
                // identity (only ws.rs has it). This arm exists so the
                // match stays exhaustive without us adding a fake outcome.
                tracing::warn!("RegisterDevice reached apply_intent — should be handled in ws.rs");
            }
            Intent::Chat { .. } | Intent::ExpandItem { .. } => {
                // Handled in ws.rs because both dispatch to the agent
                // task via the kick channel, not via state mutation.
                tracing::warn!(
                    "Chat / ExpandItem reached apply_intent — should be handled in ws.rs"
                );
            }
            Intent::SetAuthToken { .. } => {
                // Handled in ws.rs — plumbs the JWT into the mnemo token
                // store. No state-machine effect.
                tracing::warn!("SetAuthToken reached apply_intent — should be handled in ws.rs");
            }
            Intent::MintPairCode { .. } => {
                // Handled in ws.rs — talks to the DB pool for the
                // pair_codes table, not the in-memory SessionRegistry.
                tracing::warn!("MintPairCode reached apply_intent — should be handled in ws.rs");
            }
            Intent::UpsertQuickAsk { .. } | Intent::DeleteQuickAsk { .. } => {
                // Handled in ws.rs — both touch the DB. apply_intent
                // shouldn't see them.
                tracing::warn!("QuickAsk intent reached apply_intent — should be handled in ws.rs");
            }
        }
        self.assert_invariants();
        outcome
    }

    fn handle_start_meeting(
        &mut self,
        description: Option<String>,
        metadata: Option<HashMap<String, String>>,
        audio_source_device_id: Option<String>,
        assist_sensitivity: AssistSensitivity,
        outcome: &mut IntentOutcome,
    ) {
        if !matches!(self.meeting_state, MeetingState::Idle) {
            // Defense in depth: clients disable Start when a meeting
            // is already active, so a duplicate intent here means a
            // race (or a buggy/replayed client). We do NOT create a
            // second meeting — the existing one stays untouched —
            // but we re-emit `MeetingStateChanged { Active, <id> }`
            // to the originating session so its UI lands on the live
            // meeting view (cross-surface-coordination.md §
            // "Single-active-meeting enforcement"). The broadcast
            // path covers other sessions of this user already; this
            // is just to nudge the racing client.
            let active_meeting_id = self.meeting.as_ref().map(|m| m.meeting_id.clone());
            tracing::warn!(
                state = ?self.meeting_state,
                meeting_id = ?active_meeting_id,
                "start_meeting received while not idle — no-op + state re-echo"
            );
            outcome.originator_only = Some(Event::MeetingStateChanged {
                meeting_state: self.meeting_state,
                meeting_id: active_meeting_id,
            });
            return;
        }
        // Defense in depth: `handle_stop_meeting` clears every
        // non-quick_asks bucket, but a late agent write landing in the
        // idle gap can re-introduce stray items (release builds skip
        // the items-empty-when-idle debug_assert). Clear again on
        // start so a new meeting NEVER inherits a previous meeting's
        // items, whatever future writer forgets a staleness guard.
        // Same shape as the stop-time clear below; quick_asks is the
        // user's persistent library and survives meeting boundaries.
        for (mode_id, v) in self.items_per_mode.iter_mut() {
            if mode_id == "quick_asks" {
                continue;
            }
            v.clear();
        }
        // Mint the meeting id up front. Held in the runtime so `mark_moment`
        // can reference it without I/O; surfaced through the outcome
        // so the `ws` layer can persist the meetings row.
        let meeting_id = uuid::Uuid::new_v4().to_string();
        let started_wall = chrono::Utc::now();
        let mut rt = MeetingRuntime::new(meeting_id.clone(), started_wall);

        // Bind the audio source if the caller provided one. We don't
        // validate the device exists or has audio_capture today —
        // worst case the meeting runs silent. (Future: 400-equivalent
        // error when the device id is unknown.)
        rt.audio_source_device_id = audio_source_device_id.clone();
        // Persist the requested assist sensitivity on the runtime so
        // tool calls + bootstrap prompt reads pick up the right
        // value. `unwrap_or_default()` at the call-site above means
        // omitting the field == AssistSensitivity::default() ==
        // Moderate, matching the historical behavior.
        rt.assist_sensitivity = assist_sensitivity;

        self.meeting = Some(rt);
        self.meeting_state = MeetingState::Active;

        // Honor any metadata the intent supplies (manual chips set on a
        // phone or Mac before tapping start). If the intent omits metadata,
        // existing state is preserved — extraction will fill it in later via
        // `MetadataChanged`.
        if let Some(m) = metadata {
            self.metadata = m;
        }
        // Stash the user's freeform description on UserSession so the
        // agent bootstrap can pull it into the `[context]` block.
        // Empty / whitespace-only descriptions collapse to None so the
        // bootstrap doesn't emit an empty section.
        self.description = description
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        self.current_mode = DEFAULT_MODE_ID.to_string();

        outcome.events.push(Event::MeetingStateChanged {
            meeting_state: MeetingState::Active,
            meeting_id: Some(meeting_id.clone()),
        });
        outcome.events.push(Event::MetadataChanged {
            metadata: self.metadata.clone(),
        });
        outcome.events.push(Event::ModeChanged {
            mode: self.current_mode.clone(),
            display_tag: None,
            items: self.items_per_mode[&self.current_mode].clone(),
        });
        // Tell the chosen device (and any clients tracking the
        // binding) to start streaming. Only emit if a source was
        // actually bound — silent meetings with no source skip
        // this event so clients don't see redundant `None` chatter.
        if audio_source_device_id.is_some() {
            outcome.events.push(Event::AudioSourceDeviceChanged {
                device_id: audio_source_device_id,
            });
        }
        outcome.started_meeting = true;
        // Snapshot the description before it's potentially moved to
        // `start_extraction_for` below — we want both paths (DB insert
        // *and* extraction) to see the same value.
        let description_for_record = description.as_ref().filter(|s| !s.is_empty()).cloned();
        outcome.created_meeting = Some(NewMeetingRecord {
            id: meeting_id,
            started_at: started_wall,
            description: description_for_record,
            metadata: self.metadata.clone(),
            assist_sensitivity,
        });
        // Emit the initial sensitivity so other connected surfaces
        // (a phone watching a meeting started from the Mac, e.g.)
        // pick up the value without waiting for the next snapshot.
        outcome.events.push(Event::AssistSensitivityChanged {
            value: assist_sensitivity,
        });
        if let Some(d) = description.filter(|s| !s.is_empty()) {
            outcome.start_extraction_for = Some(d);
        }
    }

    fn handle_stop_meeting(&mut self, outcome: &mut IntentOutcome) {
        if matches!(self.meeting_state, MeetingState::Idle) {
            tracing::warn!("stop_meeting in idle state");
            return;
        }
        // Snapshot the transcript BEFORE we clear items_per_mode so
        // the post-meeting wrap-up extractor has the full text to
        // work with. Joined with newlines — speaker labels and
        // timestamps aren't included; the LLM extracts intent, not
        // provenance.
        let transcript_text = self
            .items_per_mode
            .get("transcript")
            .map(|items| {
                items
                    .iter()
                    .map(|i| i.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        for (mode_id, v) in self.items_per_mode.iter_mut() {
            // Quick asks are the user's persistent library — they
            // survive meeting boundaries unlike transcript / chat /
            // highlights, which are meeting-scoped.
            if mode_id == "quick_asks" {
                continue;
            }
            v.clear();
        }
        self.current_mode = DEFAULT_MODE_ID.to_string();
        // Take the runtime out — the ws layer hands it to a detached
        // `workers::finalize::run` task that gracefully drains the STT
        // pipeline, runs wrap-up on the complete transcript, then calls
        // `shutdown()` itself. Do NOT drop or cancel here; finalize owns
        // teardown. The runtime leaves the registry lock with us so the
        // detached task isn't holding it (deadlock prevention).
        let closing_meeting = self.meeting.take();
        let closing_id = closing_meeting.as_ref().map(|m| m.meeting_id.clone());
        let had_audio_source = closing_meeting
            .as_ref()
            .map(|m| m.audio_source_device_id.is_some())
            .unwrap_or(false);

        self.meeting_state = MeetingState::Idle;
        self.metadata.clear();
        self.description = None;

        outcome.events.push(Event::MeetingStateChanged {
            meeting_state: MeetingState::Idle,
            meeting_id: None,
        });
        // Tell the chosen device to stop streaming. Only emit if
        // there *was* a binding — otherwise we'd send a redundant
        // None on every plain stop.
        if had_audio_source {
            outcome
                .events
                .push(Event::AudioSourceDeviceChanged { device_id: None });
        }
        outcome.stopped_meeting = true;
        // Carry the runtime out so the ws layer can await shutdown()
        // outside the registry lock.
        outcome.stopped_runtime = closing_meeting;
        if let Some(id) = closing_id {
            outcome.closed_meeting = Some(ClosedMeetingRecord {
                id: id.clone(),
                ended_at: chrono::Utc::now(),
            });
            // Only kick the wrap-up extractor if there's actually
            // something to extract from. An empty transcript means
            // either a canceled meeting or one that ended before
            // anything was said — nothing useful for the LLM to do.
            if !transcript_text.trim().is_empty() {
                outcome.start_wrap_up = Some(WrapUpRequest {
                    meeting_id: id,
                    transcript_text,
                });
            }
        }
    }

    /// Mid-meeting flip of the assist sensitivity. Updates the
    /// runtime field so the next `PushAssistSuggestion` call (and
    /// the next agent fire, which re-builds its bootstrap section
    /// each time) uses the new value, and emits an event so other
    /// connected surfaces stay in sync. Persistence to the
    /// `meetings.assist_sensitivity` column is handled by the ws
    /// layer reading `outcome.assist_sensitivity_persist`.
    ///
    /// No-op when the meeting state is idle — the value only makes
    /// sense in the context of a live meeting (the next
    /// `start_meeting` would overwrite a stale idle-time setting
    /// anyway).
    fn handle_set_assist_sensitivity(
        &mut self,
        value: AssistSensitivity,
        outcome: &mut IntentOutcome,
    ) {
        let Some(meeting) = self.meeting.as_mut() else {
            tracing::debug!(
                "set_assist_sensitivity in idle state — no-op (next start_meeting picks the value)"
            );
            return;
        };
        if meeting.assist_sensitivity == value {
            // Idempotent re-set. Skip both broadcast and DB write so
            // a chatty client (or a snapshot-driven re-emit) doesn't
            // flood the wire.
            return;
        }
        meeting.assist_sensitivity = value;
        outcome
            .events
            .push(Event::AssistSensitivityChanged { value });
        outcome.assist_sensitivity_persist = Some(AssistSensitivityPersist {
            meeting_id: meeting.meeting_id.clone(),
            value,
        });
    }

    fn handle_set_metadata(
        &mut self,
        key: String,
        value: Option<String>,
        outcome: &mut IntentOutcome,
    ) {
        match value {
            Some(v) => {
                self.metadata.insert(key, v);
            }
            None => {
                self.metadata.remove(&key);
            }
        }
        outcome.events.push(Event::MetadataChanged {
            metadata: self.metadata.clone(),
        });
    }

    fn handle_mark_moment(
        &mut self,
        t: u64,
        note: Option<String>,
        id: Option<String>,
        self_capture: Option<bool>,
        outcome: &mut IntentOutcome,
    ) {
        if !matches!(self.meeting_state, MeetingState::Active) {
            tracing::warn!(state = ?self.meeting_state, "mark_moment in invalid state");
            return;
        }
        // `meeting_state == Active` guarantees `meeting` is Some —
        // both are set atomically in `handle_start_meeting` and cleared
        // atomically in `handle_stop_meeting`.
        let Some(meeting) = self.meeting.as_ref() else {
            tracing::error!("invariant violation: Active meeting with no MeetingRuntime");
            return;
        };
        let meeting_id = meeting.meeting_id.clone();
        // `t == 0` is the wire sentinel for "client doesn't know the
        // offset" — glasses/mobile surfaces, plus the proto3 path
        // where an absent scalar decodes as 0. Substitute the
        // server's own meeting clock. Nonzero client values are
        // trusted verbatim: across a server restart the client's
        // clock is the MORE accurate one (recovery re-stamps
        // `started_at_instant` fresh — see session/user.rs
        // rehydrate_from_recovered_meeting). Cost of the sentinel: a
        // moment genuinely marked in the meeting's first instant gets
        // a server-computed t of a few hundred ms — immaterial.
        let t = if t == 0 {
            meeting.started_at_instant.elapsed().as_millis() as u64
        } else {
            t
        };
        tracing::info!(t, ?note, meeting_id = %meeting_id, "mark_moment");
        outcome.events.push(Event::Status {
            status: Status {
                listening: true,
                error: None,
            },
        });
        outcome.mark_moment = Some(MomentRequest {
            meeting_id,
            t,
            note,
            id,
            self_capture: self_capture.unwrap_or(false),
        });
    }
}
