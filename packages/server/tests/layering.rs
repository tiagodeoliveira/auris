//! Layering ratchet: `src/ws/` is the WebSocket transport layer, not
//! the application composition root. Nothing outside it — plus
//! `src/boot.rs`, the composition root that legitimately wires the
//! transport into the app — may reference the ws module.
//!
//! The allowlist below only ever SHRINKS. If this test fails because
//! a file you touched now references `crate::ws`, import what you
//! need from its real home instead:
//!   - `ServerHandle` / `broadcast_user_event` / `EventBus` → `crate::context`
//!   - `AuthMode` / `DEV_AUTH0_SUB` → `crate::auth`
//!   - `drain_meeting_usage` / `PoolUsageRecord` → `crate::llm::usage`

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Files (relative to `src/`) still allowed to reference the ws
/// module. Shrink-only — NEVER add an entry.
/// Currently empty: the only legitimate ws consumers are `src/ws/`
/// itself and `src/boot.rs`.
const ALLOWLIST: &[&str] = &[];

/// `src/`-relative paths exempt from the scan: the ws module itself,
/// and the boot module (the composition root builds the ws router and
/// spawns ws-owned subsystems by design).
fn exempt(rel: &str) -> bool {
    rel == "boot.rs" || rel == "ws" || rel.starts_with("ws/")
}

fn scan(dir: &Path, src_root: &Path, hits: &mut BTreeSet<String>) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("dir entry").path();
        let rel = path
            .strip_prefix(src_root)
            .expect("path under src/")
            .to_string_lossy()
            .replace('\\', "/");
        if exempt(&rel) {
            continue;
        }
        if path.is_dir() {
            scan(&path, src_root, hits);
        } else if rel.ends_with(".rs") {
            let text = fs::read_to_string(&path).expect("read source file");
            if text.contains("crate::ws") || text.contains("auris_server::ws") {
                hits.insert(rel);
            }
        }
    }
}

#[test]
fn no_ws_imports_outside_ws_module() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = BTreeSet::new();
    scan(&src, &src, &mut hits);
    let allow: BTreeSet<String> = ALLOWLIST.iter().map(|s| s.to_string()).collect();

    let new_offenders: Vec<&String> = hits.difference(&allow).collect();
    assert!(
        new_offenders.is_empty(),
        "new ws-module references outside src/ws/ — import from \
         crate::context / crate::auth / crate::llm::usage instead: {new_offenders:?}"
    );

    let stale: Vec<&String> = allow.difference(&hits).collect();
    assert!(
        stale.is_empty(),
        "ALLOWLIST entries no longer reference ws — delete them so the \
         ratchet only ever shrinks: {stale:?}"
    );
}
