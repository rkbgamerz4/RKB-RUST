//! Authentication errors.
//!
//! Every message here is safe to show a user: no OAuth token, refresh token,
//! Xbox token, or request body is ever formatted into an error. HTTP failures
//! keep only the status code and a parsed, length-limited protocol message.

use std::fmt;

/// A failure during Microsoft, Xbox Live, Minecraft Services, or session-server
/// authentication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// A socket, DNS, TLS, or HTTP transport failure.
    Transport { message: String },
    /// An HTTP response that could not be interpreted as a protocol message.
    Http { status: u16, message: String },
    /// A JSON document was missing a required field or had the wrong shape.
    Json { message: String },
    /// A value was structurally valid but violates the protocol.
    Protocol { message: String },
    /// The OAuth endpoint returned a standard error code.
    OAuth { code: String, description: String },
    /// Xbox Live or XSTS rejected the account.
    Xbox {
        code: Option<i64>,
        message: String,
        redirect: Option<String>,
    },
    /// The account has no Minecraft: Java Edition profile.
    NoProfile,
    /// The account's profile exists but the entitlement store does not include
    /// Minecraft: Java Edition.
    NotOwned,
    /// The session server rejected the join request.
    JoinRejected { status: u16, message: String },
    /// A token store failed to read or write securely.
    Storage { message: String },
    /// The caller's authentication configuration is unusable.
    Config { message: String },
}

impl AuthError {
    /// A transport failure.
    #[must_use]
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport {
            message: limited(message.into()),
        }
    }

    /// An HTTP failure with a status code and a safe explanation.
    #[must_use]
    pub fn http(status: u16, message: impl Into<String>) -> Self {
        Self::Http {
            status,
            message: limited(message.into()),
        }
    }

    /// A malformed or incomplete JSON document.
    #[must_use]
    pub fn json(message: impl Into<String>) -> Self {
        Self::Json {
            message: limited(message.into()),
        }
    }

    /// A protocol violation.
    #[must_use]
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol {
            message: limited(message.into()),
        }
    }

    /// A storage failure that does not reveal file contents.
    #[must_use]
    pub fn storage(message: impl Into<String>) -> Self {
        Self::Storage {
            message: limited(message.into()),
        }
    }

    /// A caller configuration failure.
    #[must_use]
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config {
            message: limited(message.into()),
        }
    }
}

/// Keeps diagnostics short and strips control characters that could forge log
/// lines. It does not attempt to redact content; callers must never pass a
/// secret here in the first place.
fn limited(message: String) -> String {
    let mut out = String::with_capacity(message.len().min(512));
    for character in message.chars().take(512) {
        if character.is_control() {
            out.push(' ');
        } else {
            out.push(character);
        }
    }
    if message.chars().count() > 512 {
        out.push('…');
    }
    out
}

impl fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { message } => {
                write!(formatter, "authentication transport error: {message}")
            }
            Self::Http { status, message } => {
                write!(formatter, "authentication HTTP {status}: {message}")
            }
            Self::Json { message } => write!(formatter, "authentication JSON error: {message}"),
            Self::Protocol { message } => {
                write!(formatter, "authentication protocol error: {message}")
            }
            Self::OAuth { code, description } => {
                write!(formatter, "authentication rejected ({code}): {description}")
            }
            Self::Xbox {
                code,
                message,
                redirect,
            } => {
                write!(formatter, "Xbox Live rejected the sign-in")?;
                if let Some(code) = code {
                    write!(formatter, " (XErr {code})")?;
                }
                write!(formatter, ": {message}")?;
                if let Some(redirect) = redirect {
                    write!(formatter, " (see {redirect})")?;
                }
                Ok(())
            }
            Self::NoProfile => {
                write!(
                    formatter,
                    "the account has no Minecraft: Java Edition profile"
                )
            }
            Self::NotOwned => {
                write!(
                    formatter,
                    "the account does not own Minecraft: Java Edition"
                )
            }
            Self::JoinRejected { status, message } => {
                write!(
                    formatter,
                    "the session server rejected the join (HTTP {status}): {message}"
                )
            }
            Self::Storage { message } => write!(formatter, "token store error: {message}"),
            Self::Config { message } => {
                write!(formatter, "authentication configuration error: {message}")
            }
        }
    }
}

impl std::error::Error for AuthError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_length_limited_and_single_line() {
        let error = AuthError::protocol(format!("{}\nnext", "x".repeat(2000)));
        let text = error.to_string();
        assert!(text.chars().count() < 600);
        assert!(!text.contains('\n'));
    }
}
