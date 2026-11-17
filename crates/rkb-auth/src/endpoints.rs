//! Service endpoints and relying-party constants.
//!
//! The defaults are the public Microsoft, Xbox Live, Minecraft Services, and
//! session-server URLs for Minecraft: Java Edition. Tests replace them with a
//! loopback server, so no test needs the internet.

/// OAuth scopes required for an Xbox Live sign-in and a refresh token.
pub const OAUTH_SCOPE: &str = "XboxLive.signin offline_access";

/// Xbox Live user-authentication relying party.
pub const XBOX_USER_RELYING_PARTY: &str = "http://auth.xboxlive.com";

/// XSTS relying party for Minecraft Services.
pub const XSTS_RELYING_PARTY: &str = "rp://api.minecraftservices.com/";

/// Xbox Live sandbox used by retail Minecraft accounts.
pub const XBOX_SANDBOX: &str = "RETAIL";

/// Every endpoint one authentication exchange needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    /// OAuth 2.0 device-code endpoint.
    pub device_code: String,
    /// OAuth 2.0 token endpoint (device-code grant and refresh grant).
    pub token: String,
    /// Xbox Live user authentication.
    pub xbox_user_auth: String,
    /// Xbox Secure Token Service authorization.
    pub xsts: String,
    /// Minecraft Services login-with-Xbox.
    pub minecraft_login: String,
    /// Minecraft Services profile.
    pub minecraft_profile: String,
    /// Minecraft Services entitlement store.
    pub minecraft_entitlements: String,
    /// Session server join endpoint.
    pub session_join: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            device_code: "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode"
                .to_owned(),
            token: "https://login.microsoftonline.com/consumers/oauth2/v2.0/token".to_owned(),
            xbox_user_auth: "https://user.auth.xboxlive.com/user/authenticate".to_owned(),
            xsts: "https://xsts.auth.xboxlive.com/xsts/authorize".to_owned(),
            minecraft_login: "https://api.minecraftservices.com/authentication/login_with_xbox"
                .to_owned(),
            minecraft_profile: "https://api.minecraftservices.com/minecraft/profile".to_owned(),
            minecraft_entitlements: "https://api.minecraftservices.com/entitlements/mcstore"
                .to_owned(),
            session_join: "https://sessionserver.mojang.com/session/minecraft/join".to_owned(),
        }
    }
}

impl Endpoints {
    /// Rewrites every endpoint to a loopback base such as
    /// `http://127.0.0.1:8080`.
    ///
    /// This is public because it is also useful for a controlled proxy; it has
    /// no effect on the production defaults.
    #[must_use]
    pub fn with_base(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            device_code: format!("{base}/devicecode"),
            token: format!("{base}/token"),
            xbox_user_auth: format!("{base}/xbox/user"),
            xsts: format!("{base}/xbox/xsts"),
            minecraft_login: format!("{base}/minecraft/login"),
            minecraft_profile: format!("{base}/minecraft/profile"),
            minecraft_entitlements: format!("{base}/minecraft/entitlements"),
            session_join: format!("{base}/session/join"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_endpoints_are_https() {
        let endpoints = Endpoints::default();
        for url in [
            &endpoints.device_code,
            &endpoints.token,
            &endpoints.xbox_user_auth,
            &endpoints.xsts,
            &endpoints.minecraft_login,
            &endpoints.minecraft_profile,
            &endpoints.minecraft_entitlements,
            &endpoints.session_join,
        ] {
            assert!(url.starts_with("https://"), "{url}");
            assert!(!url.contains('?'), "tokens never belong in a query string");
        }
    }

    #[test]
    fn base_rewrite_keeps_paths() {
        let endpoints = Endpoints::with_base("http://127.0.0.1:9/");
        assert_eq!(endpoints.device_code, "http://127.0.0.1:9/devicecode");
        assert_eq!(endpoints.session_join, "http://127.0.0.1:9/session/join");
    }
}
