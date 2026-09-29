use thiserror::Error;

use crate::id::stable_str_enum;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoreError {
    #[error("invalid stable id {0:?}: expected 1-64 bytes of [a-z0-9_] segments joined by '.'")]
    InvalidStableId(String),
    #[error("invalid FILETIME decimal {0:?}")]
    InvalidFileTime(String),
    #[error("rate must be greater than zero")]
    ZeroRate,
    #[error("keymode must have 1-16 columns, got {0}")]
    InvalidKeymode(u8),
    #[error("column {col} is outside a {keymode}-column keymode")]
    ColumnOutOfRange { col: u8, keymode: u8 },
    #[error("invalid {type_name} hex {input:?}: expected lowercase hex of the exact length")]
    InvalidHex {
        type_name: &'static str,
        input: String,
    },
    #[error("unknown game {0:?}")]
    UnknownGame(String),
    #[error("pack section {0:?} declared twice in a version key")]
    DuplicateSection(String),
    #[error("unknown error code {0:?}")]
    UnknownErrorCode(String),
    #[error("anchor window [{t0_us}, {t1_us}) is empty or backwards")]
    InvalidAnchorWindow { t0_us: i64, t1_us: i64 },
    #[error("anchor has no columns")]
    EmptyAnchorColumns,
}

stable_str_enum! {
    /// Closed list of architecture §7; the UI localises from it, so strings never change.
    pub enum ErrorCode, unknown = CoreError::UnknownErrorCode {
        OsuDirNotFound => "OSU_DIR_NOT_FOUND",
        UnsupportedFormat => "UNSUPPORTED_FORMAT",
        ParseFailed => "PARSE_FAILED",
        OsuRunning => "OSU_RUNNING",
        ConsentRequired => "CONSENT_REQUIRED",
        SignatureInvalid => "SIGNATURE_INVALID",
        NotFound => "NOT_FOUND",
        InvalidInput => "INVALID_INPUT",
        Conflict => "CONFLICT",
        Cancelled => "CANCELLED",
        Internal => "INTERNAL",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all() {
        for code in ErrorCode::ALL {
            assert_eq!(code.as_str().parse::<ErrorCode>(), Ok(*code));
            let json = serde_json::to_string(code).unwrap();
            assert_eq!(json, format!("\"{}\"", code.as_str()));
            assert_eq!(serde_json::from_str::<ErrorCode>(&json).unwrap(), *code);
        }
        assert_eq!(
            "NOPE".parse::<ErrorCode>(),
            Err(CoreError::UnknownErrorCode("NOPE".to_owned()))
        );
        assert!("not_found".parse::<ErrorCode>().is_err());
    }

    #[test]
    fn error_code_strings() {
        let strings: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        insta::assert_snapshot!("error_code_strings", strings.join("\n"));
    }
}
