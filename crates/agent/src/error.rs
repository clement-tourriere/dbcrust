//! Error type shared by the one-shot commands and the MCP server, and the
//! process exit-code contract.

use std::fmt;

use dbcrust_agent_protocol::ErrorCode;

/// What went wrong: a broker/transport error code, or a client-local one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// Reported by the broker, or the local equivalent (`connection_error`,
    /// `protocol_error`, `timeout`)
    Remote(ErrorCode),
    /// Bad command line
    Usage,
    /// Built for a platform without Unix domain sockets
    UnsupportedPlatform,
}

impl Code {
    /// The `error.code` string printed in JSON error envelopes.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::Remote(code) => code.as_str(),
            Code::Usage => "usage_error",
            Code::UnsupportedPlatform => "unsupported_platform",
        }
    }

    /// Process exit status:
    ///
    /// | status | meaning |
    /// |---|---|
    /// | 0 | success |
    /// | 1 | `query_error` (also `unsupported_engine`, `internal`) |
    /// | 2 | usage, protocol, version or platform problem |
    /// | 3 | connection / socket / `broker_locked` |
    /// | 4 | `read_only_violation`, `ddl_refused`, `multi_statement_refused` |
    /// | 5 | `denied`, `unknown_alias`, `limit_exceeded` |
    /// | 124 | `timeout` |
    pub fn exit_code(self) -> u8 {
        match self {
            Code::Usage | Code::UnsupportedPlatform => 2,
            Code::Remote(code) => match code {
                ErrorCode::QueryError | ErrorCode::UnsupportedEngine | ErrorCode::Internal => 1,
                ErrorCode::ProtocolError | ErrorCode::UnsupportedVersion => 2,
                ErrorCode::ConnectionError | ErrorCode::BrokerLocked => 3,
                ErrorCode::ReadOnlyViolation
                | ErrorCode::DdlRefused
                | ErrorCode::MultiStatementRefused => 4,
                ErrorCode::Denied | ErrorCode::UnknownAlias | ErrorCode::LimitExceeded => 5,
                ErrorCode::Timeout => 124,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentError {
    pub code: Code,
    pub message: String,
}

impl AgentError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: Code::Remote(code),
            message: message.into(),
        }
    }

    pub fn connection(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ConnectionError, message)
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ProtocolError, message)
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            code: Code::Usage,
            message: message.into(),
        }
    }

    pub fn unsupported_platform() -> Self {
        Self {
            code: Code::UnsupportedPlatform,
            message:
                "dbcrust-agent needs Unix domain sockets and is not supported on this platform"
                    .into(),
        }
    }

    pub fn exit_code(&self) -> u8 {
        self.code.exit_code()
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AgentError {}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(ErrorCode::QueryError, 1)]
    #[case(ErrorCode::UnsupportedEngine, 1)]
    #[case(ErrorCode::Internal, 1)]
    #[case(ErrorCode::ProtocolError, 2)]
    #[case(ErrorCode::UnsupportedVersion, 2)]
    #[case(ErrorCode::ConnectionError, 3)]
    #[case(ErrorCode::BrokerLocked, 3)]
    #[case(ErrorCode::ReadOnlyViolation, 4)]
    #[case(ErrorCode::DdlRefused, 4)]
    #[case(ErrorCode::MultiStatementRefused, 4)]
    #[case(ErrorCode::Denied, 5)]
    #[case(ErrorCode::UnknownAlias, 5)]
    #[case(ErrorCode::LimitExceeded, 5)]
    #[case(ErrorCode::Timeout, 124)]
    fn exit_codes_follow_the_contract(#[case] code: ErrorCode, #[case] expected: u8) {
        assert_eq!(Code::Remote(code).exit_code(), expected);
        assert_eq!(Code::Remote(code).as_str(), code.as_str());
    }

    #[test]
    fn every_error_code_has_an_exit_code() {
        for code in ErrorCode::ALL {
            assert_ne!(Code::Remote(code).exit_code(), 0, "{code}");
        }
    }

    #[test]
    fn local_codes() {
        assert_eq!(Code::Usage.exit_code(), 2);
        assert_eq!(Code::UnsupportedPlatform.exit_code(), 2);
        assert_eq!(Code::UnsupportedPlatform.as_str(), "unsupported_platform");
    }
}
