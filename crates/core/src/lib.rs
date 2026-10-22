//! Slipstream's download engine: probing, segments, connections, storage and resume state.
//! See `docs/SPEC-engine.md`.

pub mod download;
pub mod filename;
pub mod state;
pub mod storage;

/// Version of the engine, shared by every Slipstream binary.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default `User-Agent` when the browser didn't give us one (SPEC-engine §3).
pub fn default_user_agent() -> String {
    format!("slipstream/{VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_has_name_and_version() {
        assert_eq!(default_user_agent(), format!("slipstream/{VERSION}"));
    }
}
