//! Xbox Live user authentication and XSTS authorization.
//!
//! Microsoft's OAuth token is exchanged for an Xbox Live user token, then for
//! an XSTS token. The XSTS token and the user hash (`uhs`) are what Minecraft
//! Services accepts. Both tokens are secrets; the user hash is an identifier.

use std::fmt;

use serde_json::json;
use zeroize::Zeroize;

use crate::endpoints::{Endpoints, XBOX_SANDBOX, XBOX_USER_RELYING_PARTY, XSTS_RELYING_PARTY};
use crate::error::AuthError;
use crate::http::{HttpClient, HttpRequest, HttpResponse};
use crate::json::{take_i64, take_nested_string, take_optional_string, take_required_secret};
use crate::secret::{SecretString, ZeroizingJson};

/// An XSTS token plus the Xbox user hash that identifies the account.
pub struct XboxTokens {
    xsts_token: SecretString,
    /// The `uhs` claim. Not a bearer token, but still only used in the
    /// Minecraft identity token.
    pub user_hash: String,
    /// Expiry text from XSTS, if present, for diagnostics.
    pub not_after: Option<String>,
}

impl XboxTokens {
    /// The XSTS token for Minecraft Services.
    #[must_use]
    pub(crate) fn xsts_token(&self) -> &str {
        self.xsts_token.expose()
    }
}

impl fmt::Debug for XboxTokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("XboxTokens")
            .field("xsts_token", &self.xsts_token)
            .field("user_hash_len", &self.user_hash.len())
            .field("not_after", &self.not_after)
            .finish()
    }
}

/// A user or XSTS token response.
struct XboxToken {
    token: SecretString,
    user_hash: String,
    not_after: Option<String>,
}

/// Authenticates a Microsoft access token with Xbox Live and XSTS.
pub fn authenticate(
    client: &dyn HttpClient,
    endpoints: &Endpoints,
    microsoft_access_token: &str,
) -> Result<XboxTokens, AuthError> {
    if microsoft_access_token.is_empty() {
        return Err(AuthError::config("the Microsoft access token is empty"));
    }
    let user = authenticate_user(client, endpoints, microsoft_access_token)?;
    let xsts = authorize_xsts(client, endpoints, &user)?;
    Ok(XboxTokens {
        xsts_token: xsts.token,
        user_hash: xsts.user_hash,
        not_after: xsts.not_after,
    })
}

fn authenticate_user(
    client: &dyn HttpClient,
    endpoints: &Endpoints,
    microsoft_access_token: &str,
) -> Result<XboxToken, AuthError> {
    // "d=" marks an OAuth access token for the RPS relying party.
    let mut ticket = format!("d={microsoft_access_token}");
    let value = ZeroizingJson::new(json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": ticket.as_str(),
        },
        "RelyingParty": XBOX_USER_RELYING_PARTY,
        "TokenType": "JWT",
    }));
    ticket.zeroize();
    let body = value
        .serialize()
        .map_err(|error| AuthError::json(format!("could not encode Xbox request: {error}")))?;
    let response = client.send(HttpRequest::post_json(&endpoints.xbox_user_auth, body))?;
    parse_xbox_token(response)
}

fn authorize_xsts(
    client: &dyn HttpClient,
    endpoints: &Endpoints,
    user: &XboxToken,
) -> Result<XboxToken, AuthError> {
    let value = ZeroizingJson::new(json!({
        "Properties": {
            "SandboxId": XBOX_SANDBOX,
            "UserTokens": [user.token.expose()],
        },
        "RelyingParty": XSTS_RELYING_PARTY,
        "TokenType": "JWT",
    }));
    let body = value
        .serialize()
        .map_err(|error| AuthError::json(format!("could not encode XSTS request: {error}")))?;
    let response = client.send(HttpRequest::post_json(&endpoints.xsts, body))?;
    parse_xbox_token(response)
}

fn parse_xbox_token(mut response: HttpResponse) -> Result<XboxToken, AuthError> {
    let status = response.status;
    let mut value = response.json()?;
    if status != 200 {
        return Err(xbox_error(status, &mut value));
    }
    let token = take_required_secret(&mut value, "Token")?;
    let user_hash = take_nested_string(&mut value, &["DisplayClaims", "xui", "0", "uhs"])
        .ok_or_else(|| AuthError::json("Xbox response has no DisplayClaims.xui[0].uhs"))?;
    if user_hash.is_empty() {
        return Err(AuthError::json("Xbox response has an empty user hash"));
    }
    let not_after = take_optional_string(&mut value, "NotAfter");
    Ok(XboxToken {
        token,
        user_hash,
        not_after,
    })
}

fn xbox_error(status: u16, value: &mut serde_json::Value) -> AuthError {
    let code = take_i64(value, "XErr");
    let message = take_optional_string(value, "Message")
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| format!("HTTP {status}"));
    let redirect = take_optional_string(value, "Redirect");
    AuthError::Xbox {
        code,
        message,
        redirect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::MockClient;

    fn response(status: u16, body: &str) -> HttpResponse {
        HttpResponse::new(status, body.as_bytes().to_vec())
    }

    #[test]
    fn user_and_xsts_requests_carry_the_right_claims() {
        let client = MockClient::new(vec![
            response(
                200,
                r#"{"Token":"user-token","NotAfter":"soon","DisplayClaims":{"xui":[{"uhs":"user-hash"}]}}"#,
            ),
            response(
                200,
                r#"{"Token":"xsts-token","NotAfter":"later","DisplayClaims":{"xui":[{"uhs":"user-hash"}]}}"#,
            ),
        ]);
        let tokens = authenticate(&client, &Endpoints::default(), "ms-access").unwrap();
        assert_eq!(tokens.user_hash, "user-hash");
        assert_eq!(tokens.xsts_token(), "xsts-token");

        let requests = client.take_requests();
        assert_eq!(requests.len(), 2);
        let user_body = requests[0].body_text().unwrap();
        assert!(user_body.contains(r#""RpsTicket":"d=ms-access""#));
        assert!(user_body.contains(r#""AuthMethod":"RPS""#));
        assert!(user_body.contains(XBOX_USER_RELYING_PARTY));

        let xsts_body = requests[1].body_text().unwrap();
        assert!(xsts_body.contains(r#""UserTokens":["user-token"]"#));
        assert!(xsts_body.contains(r#""SandboxId":"RETAIL""#));
        assert!(xsts_body.contains(XSTS_RELYING_PARTY));
    }

    #[test]
    fn xsts_errors_expose_the_code_but_not_tokens() {
        let client = MockClient::new(vec![
            response(
                200,
                r#"{"Token":"user-token","DisplayClaims":{"xui":[{"uhs":"h"}]}}"#,
            ),
            response(
                401,
                r#"{"Identity":"0","XErr":2148916233,"Message":"No Xbox account","Redirect":"https://start.ui.xboxlive.com/CreateAccount"}"#,
            ),
        ]);
        let error = authenticate(&client, &Endpoints::default(), "ms-access").unwrap_err();
        let text = error.to_string();
        assert!(text.contains("2148916233"));
        assert!(text.contains("No Xbox account"));
        assert!(!text.contains("user-token"));
        assert!(!text.contains("ms-access"));
    }

    #[test]
    fn missing_uhs_is_rejected() {
        let client = MockClient::new(vec![response(200, r#"{"Token":"t"}"#)]);
        assert!(authenticate(&client, &Endpoints::default(), "ms").is_err());
    }

    #[test]
    fn debug_redacts_tokens() {
        let tokens = XboxTokens {
            xsts_token: SecretString::new("xsts-secret"),
            user_hash: "hash".to_owned(),
            not_after: None,
        };
        let text = format!("{tokens:?}");
        assert!(!text.contains("xsts-secret"));
        assert!(text.contains("user_hash_len"));
    }
}
