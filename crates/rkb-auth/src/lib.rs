//! Microsoft, Xbox Live, and Minecraft Services authentication.
//!
//! This crate signs a player in without asking for a password and without
//! logging credentials:
//!
//! ```text
//! OAuth device code → Xbox Live user token → XSTS token
//!   → Minecraft Services token → profile → entitlements → session join
//! ```
//!
//! [`flow`] orchestrates the exchange, [`session::OnlineAuthenticator`]
//! implements `rkb-net`'s `SessionAuthenticator` so an online-mode login can
//! join a server, and [`store`] optionally persists a refresh token.
//!
//! # Security model
//!
//! - No password is collected. The user approves a device code in a browser.
//! - RKB does not ship an Azure client id or a client secret. The caller
//!   supplies a registered application id.
//! - Access and refresh tokens are [`secret::SecretString`]: `Debug` prints
//!   `<redacted>` and the buffer is zeroed on drop. HTTP bodies are zeroed as
//!   well.
//! - Tokens are sent in request bodies or `Authorization` headers, never in a
//!   query string, and never written to logs or error messages.
//! - Refresh-token persistence is opt-in and owner-only (`0600`).
//!
//! # Example
//!
//! ```no_run
//! use std::time::Duration;
//! use rkb_auth::{flow, AuthConfig, UreqClient};
//!
//! let client = UreqClient::new(Duration::from_secs(20));
//! let config = AuthConfig::new("00000000-0000-0000-0000-000000000000");
//! let start = flow::begin_device_code(&client, &config)?;
//! println!("open {} and enter {}", start.verification_uri, start.user_code);
//! let signed = flow::finish_device_code(&client, &config, &start, rkb_auth::thread_sleep)?;
//! println!("signed in as {}", signed.session.name());
//! # Ok::<(), rkb_auth::AuthError>(())
//! ```

#![forbid(unsafe_code)]

pub mod endpoints;
pub mod error;
pub mod flow;
pub mod http;
mod json;
pub mod minecraft;
pub mod oauth;
pub mod secret;
pub mod session;
pub mod store;
pub mod xbox;

#[cfg(test)]
mod test_support;

pub use endpoints::Endpoints;
pub use error::AuthError;
pub use flow::{AuthConfig, SignedIn};
pub use http::{HttpClient, UreqClient};
pub use minecraft::{Entitlements, MinecraftProfile, MinecraftToken};
pub use oauth::{DeviceCodeStart, MicrosoftToken, PollOutcome};
pub use secret::SecretString;
pub use session::{MinecraftSession, OnlineAuthenticator};
pub use store::{FileTokenStore, MemoryTokenStore, StoredSession, TokenStore};

/// Blocks the current thread between device-code polls. Passed to
/// [`flow::finish_device_code`].
pub use oauth::thread_sleep;
