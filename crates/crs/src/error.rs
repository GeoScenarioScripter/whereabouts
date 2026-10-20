//! Error type for CRS parsing.

/// Why a CRS definition could not be parsed.
///
/// CRS text comes from user files (`.prj`, GeoPackage tables, GeoParquet metadata), so every
/// failure mode — truncation, garbage bytes, absurd nesting, nonsensical numbers — is reported
/// through this type and never as a panic. `offset` fields are byte offsets into the input text.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum CrsError {
    /// The input is empty or contains only whitespace / NUL padding.
    #[error("empty CRS definition")]
    Empty,

    /// The input is larger than [`crate::MAX_INPUT_LEN`].
    #[error("CRS definition too large: {len} bytes (limit {limit})")]
    TooLarge {
        /// Input length in bytes.
        len: usize,
        /// The enforced limit in bytes.
        limit: usize,
    },

    /// WKT brackets nest deeper than [`crate::MAX_DEPTH`].
    #[error("WKT nesting deeper than {limit} levels at byte {offset}")]
    TooDeep {
        /// Byte offset of the bracket that exceeded the limit.
        offset: usize,
        /// The enforced nesting limit.
        limit: usize,
    },

    /// The WKT contains more than [`crate::MAX_NODES`] keyword nodes.
    #[error("WKT has more than {limit} nodes")]
    TooManyNodes {
        /// The enforced node limit.
        limit: usize,
    },

    /// The WKT is not syntactically valid (unterminated string, missing bracket, bad number, …).
    #[error("WKT syntax error at byte {offset}: {message}")]
    Syntax {
        /// Byte offset where the problem was detected.
        offset: usize,
        /// What was wrong.
        message: String,
    },

    /// The root keyword is not a CRS keyword this crate knows.
    #[error("unsupported or unknown CRS keyword `{0}`")]
    UnknownRoot(String),

    /// A required element is missing (e.g. a `PROJCS` without `PROJECTION`).
    #[error("missing {element} in {context}")]
    Missing {
        /// The element that was expected.
        element: &'static str,
        /// Where it was expected.
        context: String,
    },

    /// An element is present but its value is invalid (non-positive semi-major axis, …).
    #[error("invalid {element}: {message}")]
    Invalid {
        /// The offending element.
        element: &'static str,
        /// What was wrong with it.
        message: String,
    },

    /// The PROJJSON document is not valid JSON or does not follow the PROJJSON structure.
    #[error("PROJJSON: {0}")]
    Json(String),
}

impl CrsError {
    pub(crate) fn syntax(offset: usize, message: impl Into<String>) -> Self {
        Self::Syntax {
            offset,
            message: message.into(),
        }
    }

    pub(crate) fn missing(element: &'static str, context: impl Into<String>) -> Self {
        Self::Missing {
            element,
            context: context.into(),
        }
    }

    pub(crate) fn invalid(element: &'static str, message: impl Into<String>) -> Self {
        Self::Invalid {
            element,
            message: message.into(),
        }
    }
}
