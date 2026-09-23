use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("xml error: {0}")]
    Xml(#[from] roxmltree::Error),

    /// The user has not authorised yet, or the stored token is gone/expired.
    #[error("not authenticated")]
    NotAuthenticated,

    #[error("device authorisation failed: {0}")]
    DeviceAuth(String),

    #[error("api error: {0}")]
    Api(String),

    #[error("tauri error: {0}")]
    Tauri(#[from] tauri::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

// Tauri commands need a serialisable error; the frontend only ever shows the message.
impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
