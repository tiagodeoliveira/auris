//! Tiny env-var helpers used across the crate.
//!
//! The "is this boolean flag set?" pattern used to be
//! `std::env::var("X").is_ok()`, which silently considered an
//! empty-string value as "set" — exactly what docker-compose's
//! `${VAR:-}` substitution produces when the operator hasn't put
//! the variable in `.env.deploy`. Switching every callsite to
//! `flag(...)` (which treats empty values as unset) fixes the
//! deploy-time toggle drift without changing how operators
//! explicitly opt in (`VAR=1` still works as before).

/// True iff the given env var is set AND has a non-empty value.
/// Use for boolean toggles that should be off when the operator
/// hasn't explicitly opted in. Matches the behavior every callsite
/// _intended_ before the docker-compose env-passthrough exposed the
/// empty-string-counts-as-set gotcha.
pub fn flag(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Read an env var, returning `default` if it's unset OR an empty
/// string. The empty-string case matters because docker-compose's
/// `${VAR:-}` substitution lands in the container as `KEY=""` which
/// `std::env::var().unwrap_or_else(|_| ...)` treats as "set" — that
/// silently took an empty `MEETING_COMPANION_LLM_MODEL_ID` straight
/// to the Anthropic API, which rejected with "model: String should
/// have at least 1 character". Use this helper for any "stringly-
/// typed knob with a sensible default" — model ids, regions, file
/// paths, etc.
pub fn var_or(key: &str, default: &str) -> String {
    var_opt(key).unwrap_or_else(|| default.to_string())
}

/// Read an env var as `Some(value)` if it's set AND non-empty.
/// Use for optional credentials / endpoints where empty means
/// "operator chose not to configure this" (e.g., Soniox API key
/// absent → STT disabled; mnemo URL absent → memory layer off).
pub fn var_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test mutates and restores the same env-var slot. The
    /// suite runs with `--test-threads=1` (per the heartbeat tests'
    /// pre-existing constraint), so the lack of inter-test isolation
    /// is fine.
    const TEST_KEY: &str = "MEETING_COMPANION_ENV_FLAG_TEST";

    fn with_var<F: FnOnce()>(value: Option<&str>, body: F) {
        match value {
            Some(v) => std::env::set_var(TEST_KEY, v),
            None => std::env::remove_var(TEST_KEY),
        }
        body();
        std::env::remove_var(TEST_KEY);
    }

    #[test]
    fn unset_is_false() {
        with_var(None, || assert!(!flag(TEST_KEY)));
    }

    #[test]
    fn empty_is_false() {
        // The docker-compose `${VAR:-}` substitution lands here.
        with_var(Some(""), || assert!(!flag(TEST_KEY)));
    }

    #[test]
    fn nonempty_is_true() {
        with_var(Some("1"), || assert!(flag(TEST_KEY)));
        with_var(Some("true"), || assert!(flag(TEST_KEY)));
        // Anything non-empty trips the flag; the toggle is a
        // "set or not" knob, not a boolean parser.
        with_var(Some("anything"), || assert!(flag(TEST_KEY)));
    }
}
