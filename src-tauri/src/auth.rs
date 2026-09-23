//! OAuth device-code flow. Yandex registers no third-party apps, so we present
//! the official Android app's credentials; the password never reaches us.
//! See ANALYSIS.md §1.

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const CLIENT_ID: &str = "23cabbbdc6cd418abb4b39c32c41195d";
const CLIENT_SECRET: &str = "53bc75238f0c4d08a118e51fe9203300";
const OAUTH_BASE: &str = "https://oauth.yandex.ru";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthToken {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub token_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthError {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

fn random_device_id() -> String {
    const ALPHANUM: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..10)
        .map(|_| ALPHANUM[rng.gen_range(0..ALPHANUM.len())] as char)
        .collect()
}

/// Step 1: ask Yandex for a code the user will confirm in their browser.
pub async fn request_device_code(http: &reqwest::Client, device_name: &str) -> Result<DeviceCode> {
    let resp = http
        .post(format!("{OAUTH_BASE}/device/code"))
        .form(&[
            ("client_id", CLIENT_ID),
            ("device_id", &random_device_id()),
            ("device_name", device_name),
        ])
        .send()
        .await?;

    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(Error::DeviceAuth(format!("device/code returned {status}: {body}")));
    }
    serde_json::from_str(&body).map_err(|e| Error::DeviceAuth(format!("bad device/code response: {e}")))
}

/// Step 2: poll once. `Ok(None)` means the user has not confirmed yet.
pub async fn poll_device_token(
    http: &reqwest::Client,
    device_code: &str,
) -> Result<Option<OAuthToken>> {
    let resp = http
        .post(format!("{OAUTH_BASE}/token"))
        .form(&[
            ("grant_type", "device_code"),
            ("code", device_code),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ])
        .send()
        .await?;

    let status = resp.status();
    let body = resp.text().await?;

    if status.is_success() {
        return Ok(Some(serde_json::from_str(&body)?));
    }

    // Yandex reports "still waiting" as a 400 with a specific error code.
    match serde_json::from_str::<OAuthError>(&body) {
        Ok(e) if e.error == "authorization_pending" => Ok(None),
        Ok(e) => Err(Error::DeviceAuth(
            e.error_description.unwrap_or(e.error),
        )),
        Err(_) => Err(Error::DeviceAuth(format!("token returned {status}: {body}"))),
    }
}

/// Exchange a refresh token for a fresh access token.
pub async fn refresh_token(http: &reqwest::Client, refresh: &str) -> Result<OAuthToken> {
    let resp = http
        .post(format!("{OAUTH_BASE}/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ])
        .send()
        .await?;

    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(Error::DeviceAuth(format!("refresh returned {status}: {body}")));
    }
    Ok(serde_json::from_str(&body)?)
}
