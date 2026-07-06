//! Device registry methods for `UserSession`.

use crate::session::UserSession;

impl UserSession {
    /// Register (or re-register) a device under the given WS
    /// connection. Returns the assigned device.
    ///
    /// `requested_id` carries a client-supplied stable device id. When
    /// present we reuse it (so a browser reconnecting after a network
    /// switch keeps its identity and its audio-source binding), and we
    /// evict any *other* connection still holding that same id — the
    /// stale socket from before the reconnect, which the server hasn't
    /// noticed is dead yet. Without that eviction the devices list
    /// would briefly show two entries for one logical device. Absent
    /// → mint a fresh UUID (Mac / older clients).
    pub fn register_device(
        &mut self,
        connection_id: String,
        hostname: String,
        capabilities: Vec<crate::protocol::Capability>,
        requested_id: Option<String>,
    ) -> crate::protocol::Device {
        let id = requested_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        // Drop any stale connection bound to this same logical device
        // (a reconnect of the same id under a new connection).
        self.devices_by_connection
            .retain(|conn, d| !(d.id == id && conn != &connection_id));
        let device = crate::protocol::Device {
            id,
            hostname,
            capabilities,
            online: true,
        };
        self.devices_by_connection
            .insert(connection_id, device.clone());
        device
    }

    /// Remove a device when its WS connection closes. Returns the
    /// removed device (for diagnostics) if there was one.
    pub fn unregister_device(&mut self, connection_id: &str) -> Option<crate::protocol::Device> {
        // Remove the connection's device entry, but DELIBERATELY leave
        // `meeting.audio_source_device_id` intact. The binding must
        // survive a disconnect so the same device — reconnecting after
        // a crash / force-quit / Ctrl-C — can RESUME the live meeting:
        // the snapshot carries the binding, the client matches it
        // against its own stable device id, and re-opens /audio.
        //
        // We used to clear it here when the bound device was "truly
        // gone", but a kill→reopen is exactly a disconnect-then-
        // reconnect gap: the old connection unregisters (clearing the
        // binding) BEFORE the new one registers, so the reconnecting
        // device finds itself unbound and can't resume. Genuine
        // abandonment — a source that never returns — is handled by the
        // liveness reaper, which ends the meeting after the audio grace
        // window. So clearing here is both harmful (breaks resume) and
        // redundant (the reaper is the real safety net).
        self.devices_by_connection.remove(connection_id)
    }

    /// Snapshot of all currently-registered devices.
    pub fn devices_clone(&self) -> Vec<crate::protocol::Device> {
        self.devices_by_connection.values().cloned().collect()
    }
}
