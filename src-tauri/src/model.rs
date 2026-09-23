//! Serde models for the subset of the API this player uses.
//! Only the fields we actually consume are modelled; the API sends far more.

use serde::{Deserialize, Deserializer, Serialize};

/// Track ids come back as either a JSON string or a number depending on endpoint.
fn flexible_id<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    })
}

fn flexible_id_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Option::<serde_json::Value>::deserialize(d)? {
        Some(serde_json::Value::String(s)) => Some(s),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(serde_json::Value::Null) | None => None,
        Some(other) => Some(other.to_string()),
    })
}

#[derive(Debug, Clone, Deserialize)]
pub struct Named {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    #[serde(deserialize_with = "flexible_id")]
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub artists: Vec<Named>,
    #[serde(default)]
    pub albums: Vec<Named>,
    #[serde(default)]
    pub cover_uri: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub available: Option<bool>,
    /// Podcast episodes only: ISO date of release.
    #[serde(default)]
    pub pub_date: Option<String>,
}

/// `/search`; only the block matching the requested `type` is present, and
/// none at all when nothing matched.
#[derive(Debug, Deserialize)]
pub struct SearchResult {
    #[serde(default)]
    pub tracks: Option<SearchBlock<Track>>,
    #[serde(default)]
    pub podcasts: Option<SearchBlock<Album>>,
}

#[derive(Debug, Deserialize)]
pub struct SearchBlock<T> {
    #[serde(default = "Vec::new")]
    pub results: Vec<T>,
}

/// A podcast is an album of `type: "podcast"` whose tracks are episodes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    #[serde(deserialize_with = "flexible_id")]
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub cover_uri: Option<String>,
    #[serde(default)]
    pub track_count: Option<u32>,
    /// `albums/{id}/with-tracks` only: episodes in groups, newest first.
    #[serde(default)]
    pub volumes: Vec<Vec<Track>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastView {
    pub id: String,
    pub title: String,
    pub episode_count: u32,
    pub cover_thumb_url: Option<String>,
}

impl From<&Album> for PodcastView {
    fn from(a: &Album) -> Self {
        PodcastView {
            id: a.id.clone(),
            title: a.title.clone().unwrap_or_else(|| "Untitled".into()),
            episode_count: a.track_count.unwrap_or(0),
            cover_thumb_url: a.cover_uri.as_ref().map(|u| cover_url(u, 100)),
        }
    }
}

/// Flattened shape handed to the frontend, and back from it to play a
/// search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackView {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_url: Option<String>,
    pub cover_thumb_url: Option<String>,
    pub duration_ms: u64,
    pub available: bool,
    #[serde(default)]
    pub pub_date: Option<String>,
}

impl From<&Track> for TrackView {
    fn from(t: &Track) -> Self {
        let artist = t
            .artists
            .iter()
            .filter_map(|a| a.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        TrackView {
            id: t.id.clone(),
            title: t.title.clone().unwrap_or_else(|| "Unknown".into()),
            artist: if artist.is_empty() { "Unknown artist".into() } else { artist },
            album: t.albums.first().and_then(|a| a.title.clone()).unwrap_or_default(),
            cover_url: t.cover_uri.as_ref().map(|u| cover_url(u, 400)),
            cover_thumb_url: t.cover_uri.as_ref().map(|u| cover_url(u, 100)),
            duration_ms: t.duration_ms.unwrap_or(0),
            available: t.available.unwrap_or(true),
            pub_date: t.pub_date.clone(),
        }
    }
}

/// `coverUri` arrives as `avatars.../%%` with no scheme and a size placeholder.
pub fn cover_url(uri: &str, size: u32) -> String {
    format!("https://{}", uri.replace("%%", &format!("{size}x{size}")))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    #[serde(default, deserialize_with = "flexible_id_opt")]
    pub uid: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub login: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plus {
    #[serde(default)]
    pub has_plus: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub account: Account,
    #[serde(default)]
    pub plus: Option<Plus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub uid: String,
    pub display_name: String,
    pub has_plus: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadInfo {
    pub codec: String,
    #[serde(default)]
    pub bitrate_in_kbps: Option<u32>,
    #[serde(default)]
    pub preview: Option<bool>,
    pub download_info_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SequenceItem {
    #[serde(default)]
    pub track: Option<Track>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationTracks {
    #[serde(default)]
    pub batch_id: Option<String>,
    #[serde(default)]
    pub sequence: Vec<SequenceItem>,
}

// ---- rotor stations ----

/// Station ids arrive split; the endpoints want them joined as `type:tag`.
#[derive(Debug, Clone, Deserialize)]
pub struct StationId {
    #[serde(rename = "type")]
    pub kind: String,
    pub tag: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationIcon {
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub background_color: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Station {
    pub id: StationId,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<StationIcon>,
}

/// Both listings wrap the station alongside its settings and ad params.
#[derive(Debug, Clone, Deserialize)]
pub struct StationEntry {
    pub station: Station,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StationsDashboard {
    #[serde(default)]
    pub stations: Vec<StationEntry>,
}

/// Flattened shape handed to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StationView {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub icon_url: Option<String>,
    pub color: Option<String>,
    /// Came from the personalised dashboard rather than the full catalogue.
    pub featured: bool,
}

impl From<&Station> for StationView {
    fn from(s: &Station) -> Self {
        let icon = s.icon.as_ref();
        StationView {
            id: format!("{}:{}", s.id.kind, s.id.tag),
            kind: s.id.kind.clone(),
            name: s.name.clone().unwrap_or_else(|| s.id.tag.clone()),
            // Station art uses the same scheme-less `%%` convention as covers.
            icon_url: icon
                .and_then(|i| i.image_url.as_ref())
                .map(|u| cover_url(u, 100)),
            color: icon.and_then(|i| i.background_color.clone()),
            featured: false,
        }
    }
}
