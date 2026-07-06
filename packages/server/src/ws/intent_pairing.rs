//! Pair-flow intents — `Intent::MintPairCode` (issue an OTP for QR
//! pairing) and `Intent::RegisterDevice` (an already-paired device
//! announcing its presence on a fresh connection).
//!
//! Both touch device identity rather than meeting state, so they
//! share this module out of `ws/control.rs`'s `dispatch_intent`.

use anyhow::Result;
use axum::extract::ws::Message;
use futures_util::SinkExt;

use crate::protocol::Event;

use super::control::WsSender;
use crate::context::ServerHandle;

/// `Intent::MintPairCode` — issue a fresh pair-code via the DB and
/// send the code back to the originating client only. The HTTP
/// `/pair/code` endpoint stays alive for backwards compat. Direct-
/// only response: the code is sensitive enough that we don't want
/// it broadcast across the user's other surfaces.
pub(super) async fn handle_mint_pair_code(
    handle: &ServerHandle,
    user_id: &str,
    sink: &mut WsSender,
) -> Result<()> {
    // device_label is intentionally ignored at mint time: per the
    // pairing spec, the label is set when the device redeems (it's
    // "what this device wants to be called"), not by the mobile
    // minter. Field carried in the intent for forward-compat in case
    // we move that semantic.
    tracing::info!(user_id = %user_id, "mint_pair_code received");
    match crate::auth::pairing::mint_code(&handle.db, user_id).await {
        Ok(minted) => {
            tracing::info!(
                user_id = %user_id,
                code_prefix = %&minted.code[..3],
                "pair_code_minted"
            );
            let event = Event::PairCodeMinted {
                code: crate::auth::pairing::format_code(&minted.code),
                expires_at: minted.expires_at.to_rfc3339(),
            };
            sink.send(Message::Text(serde_json::to_string(&event)?))
                .await
                .ok();
        }
        Err(e) => {
            tracing::warn!(error = %e, user_id = %user_id, "mint_pair_code failed");
            let event = Event::Error {
                code: "pair_mint_failed".to_string(),
                message: e.to_string(),
                intent_ref: None,
            };
            sink.send(Message::Text(serde_json::to_string(&event)?))
                .await
                .ok();
        }
    }
    Ok(())
}

/// `Intent::RegisterDevice` — register this connection as a known
/// device under the authed user. Sends a direct `DeviceRegistered`
/// to the originating client so it learns its assigned `device_id`,
/// then broadcasts the updated devices list to all of *this user's*
/// connections.
///
/// Needs the connection_id (only the WS handler has it), so it
/// lives here rather than in `apply_intent`.
pub(super) async fn handle_register_device(
    handle: &ServerHandle,
    user_id: &str,
    connection_id: &str,
    sink: &mut WsSender,
    hostname: String,
    capabilities: Vec<crate::protocol::Capability>,
    device_id: Option<String>,
) -> Result<()> {
    let (device, all_devices) = {
        let mut s = handle.sessions.lock().await;
        let device = s.register_device(
            user_id,
            connection_id.to_string(),
            hostname,
            capabilities,
            device_id,
        );
        let all = s.devices_clone_for(user_id);
        (device, all)
    };
    // Direct response to the registering client (so it learns its
    // own assigned device_id). Sent on the auth'd sink before any
    // broadcast lands, so the client never sees its own device in
    // a `DevicesChanged` before the `DeviceRegistered`.
    let registered = Event::DeviceRegistered { device };
    sink.send(Message::Text(serde_json::to_string(&registered)?))
        .await
        .ok();
    // Fan out the new devices list to *this user's* connections only.
    handle
        .bus
        .emit(
            user_id.to_string(),
            Event::DevicesChanged {
                devices: all_devices,
            },
        )
        .await;
    Ok(())
}
