//! Add-ons a build may leave out.
//!
//! Engines are gated in `dbcrust-drivers` (`DatabaseError::ConnectorNotBuilt`).
//! The add-ons that are not engines — Docker discovery, the AI assistant, the
//! plan/schema TUIs and the clipboard — are cargo features of the `dbcrust` crate
//! (`docker`, `ai`, `explain-tui`, `clipboard`); this error is what their entry
//! points report when the feature is off. The `\ai`, `??`, `\docker` … commands
//! still parse, so the REPL can say what is missing instead of "unknown command".

use thiserror::Error;

/// An add-on this build was compiled without. The message names the cargo
/// feature that provides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FeatureNotCompiled {
    #[error(
        "Docker support is not compiled into this build (enable the `docker` cargo feature, or install a full build)"
    )]
    Docker,
    #[error(
        "AI support is not compiled into this build (enable the `ai` cargo feature, or install a full build)"
    )]
    Ai,
    #[error(
        "The explain/schema TUI is not compiled into this build (enable the `explain-tui` cargo feature, or install a full build)"
    )]
    ExplainTui,
    #[error(
        "Clipboard support is not compiled into this build (enable the `clipboard` cargo feature, or install a full build)"
    )]
    Clipboard,
}

impl FeatureNotCompiled {
    /// The cargo feature that provides the add-on.
    pub fn feature(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Ai => "ai",
            Self::ExplainTui => "explain-tui",
            Self::Clipboard => "clipboard",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(FeatureNotCompiled::Docker, "docker", "Docker support is not compiled")]
    #[case(FeatureNotCompiled::Ai, "ai", "AI support is not compiled")]
    #[case(FeatureNotCompiled::ExplainTui, "explain-tui", "TUI is not compiled")]
    #[case(
        FeatureNotCompiled::Clipboard,
        "clipboard",
        "Clipboard support is not compiled"
    )]
    fn messages_name_the_cargo_feature(
        #[case] error: FeatureNotCompiled,
        #[case] feature: &str,
        #[case] start: &str,
    ) {
        assert_eq!(error.feature(), feature);
        let text = error.to_string();
        assert!(text.contains(start), "{text}");
        assert!(
            text.contains(&format!("`{feature}` cargo feature")),
            "{text}"
        );
    }
}
