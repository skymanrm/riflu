//! Thin typed client over the unofficial Yandex Music API.
//! One place owns the auth header and the 401 refresh.

use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::auth::{self, OAuthToken};
use crate::error::{Error, Result};
use crate::media;
use crate::model::*;
use crate::store;

const BASE: &str = "https://api.music.yandex.net";
const MUSIC_CLIENT: &str = "YandexMusicAndroid/24023621";

/// Every endpoint wraps its payload in an envelope.
#[derive(Deserialize)]
struct Envelope<T> {
    result: T,
}

/// Request bodies differ per subsystem: the rotor rejects form encoding and
/// wants JSON, while the older endpoints still take form data.
pub enum Body {
    Form(Vec<(String, String)>),
    Json(serde_json::Value),
}

pub struct Api {
    http: reqwest::Client,
    token: RwLock<OAuthToken>,
}

impl Api {
    pub fn new(token: OAuthToken) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("riflu/0.1")
                .build()
                .expect("http client"),
            token: RwLock::new(token),
        }
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    async fn access_token(&self) -> String {
        self.token.read().await.access_token.clone()
    }

    fn request(
        &self,
        method: reqwest::Method,
        url: &str,
        token: &str,
    ) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .header("Authorization", format!("OAuth {token}"))
            .header("X-Yandex-Music-Client", MUSIC_CLIENT)
            .header("Accept-Language", "ru")
    }

    /// Swap the access token using the refresh token, and persist the new pair.
    async fn try_refresh(&self) -> Result<bool> {
        let refresh = self.token.read().await.refresh_token.clone();
        let Some(refresh) = refresh else {
            return Ok(false);
        };
        let fresh = auth::refresh_token(&self.http, &refresh).await?;
        store::save(&fresh)?;
        *self.token.write().await = fresh;
        Ok(true)
    }

    /// Send a request, refreshing once on 401 before giving up.
    async fn send<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<&Body>,
    ) -> Result<T> {
        for attempt in 0..2 {
            let token = self.access_token().await;
            let mut req = self.request(method.clone(), url, &token);
            match body {
                Some(Body::Form(f)) => req = req.form(f),
                Some(Body::Json(j)) => req = req.json(j),
                None => {}
            }
            let resp = req.send().await?;
            let status = resp.status();

            if status == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                if self.try_refresh().await? {
                    continue;
                }
                return Err(Error::NotAuthenticated);
            }
            let body = resp.text().await?;
            if !status.is_success() {
                return Err(Error::Api(format!("{method} {url} -> {status}: {}", truncate(&body))));
            }
            return decode(url, &body);
        }
        Err(Error::NotAuthenticated)
    }

    async fn get<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        self.send(reqwest::Method::GET, url, None).await
    }

    // ---- account ----

    pub async fn account_status(&self) -> Result<AccountView> {
        let s: AccountStatus = self.get(&format!("{BASE}/account/status")).await?;
        Ok(AccountView {
            uid: s.account.uid.clone().unwrap_or_default(),
            display_name: s
                .account
                .display_name
                .or(s.account.login)
                .unwrap_or_else(|| "Unknown".into()),
            has_plus: s.plus.and_then(|p| p.has_plus).unwrap_or(false),
        })
    }

    // ---- playback ----

    /// Resolve a playable URL. Stream URLs are timestamp-signed and expire, so
    /// this must be called per play and never cached.
    pub async fn track_url(&self, track_id: &str) -> Result<String> {
        let infos: Vec<DownloadInfo> = self
            .get(&format!("{BASE}/tracks/{track_id}/download-info"))
            .await?;

        let best = media::pick_best(&infos)
            .ok_or_else(|| Error::Api(format!("no downloadable variant for track {track_id}")))?;

        // The XML lives on a signed URL that needs no auth header.
        let xml = self.http.get(&best.download_info_url).send().await?.text().await?;
        media::build_direct_link(&xml)
    }

    /// Music search. `type=all` answers every block at once (artists and
    /// albums 10 each, tracks 20); only those three are kept. [V]
    pub async fn search_all(&self, text: &str) -> Result<SearchAllView> {
        let res = self.search(text, "all").await?;
        Ok(SearchAllView {
            artists: SearchBlock::views(&res.artists),
            albums: SearchBlock::views(&res.albums),
            tracks: SearchBlock::views(&res.tracks),
        })
    }

    /// An artist's tracks, most popular first. [V]
    pub async fn artist_tracks(&self, id: &str) -> Result<Vec<TrackView>> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            tracks: Vec<Track>,
        }
        let page: Page = self
            .get(&format!("{BASE}/artists/{id}/tracks?page=0&page-size=50"))
            .await?;
        Ok(page.tracks.iter().map(TrackView::from).collect())
    }

    /// `type=podcast` answers `{podcasts: {results}}` of podcast albums. [V]
    pub async fn search_podcasts(&self, text: &str) -> Result<Vec<PodcastView>> {
        let res = self.search(text, "podcast").await?;
        let podcasts = res.podcasts.map(|p| p.results).unwrap_or_default();
        Ok(podcasts.iter().map(PodcastView::from).collect())
    }

    async fn search(&self, text: &str, kind: &str) -> Result<SearchResult> {
        let url = reqwest::Url::parse_with_params(
            &format!("{BASE}/search"),
            &[("text", text), ("type", kind), ("page", "0"), ("nocorrect", "false")],
        )
        .map_err(|e| Error::Api(format!("bad search url: {e}")))?;
        self.get(url.as_str()).await
    }

    /// Every track of an album, or every episode of a podcast, newest first.
    /// `with-tracks` returns them all in one page (500 checked), split into
    /// volumes that are flattened here. Episodes carry no artist, so the
    /// podcast's title stands in. [V]
    pub async fn album_tracks(&self, id: &str) -> Result<Vec<TrackView>> {
        let album: Album = self.get(&format!("{BASE}/albums/{id}/with-tracks")).await?;
        let show = album.title.clone().unwrap_or_default();
        Ok(album
            .volumes
            .iter()
            .flatten()
            .map(|t| {
                let mut v = TrackView::from(t);
                if t.artists.is_empty() {
                    v.artist = show.clone();
                }
                v
            })
            .collect())
    }

    pub async fn tracks(&self, ids: &[String]) -> Result<Vec<TrackView>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let body = Body::Form(vec![("trackIds".into(), ids.join(","))]);
        let tracks: Vec<Track> = self
            .send(reqwest::Method::POST, &format!("{BASE}/tracks"), Some(&body))
            .await?;
        Ok(tracks.iter().map(TrackView::from).collect())
    }

    // ---- rotor / waves ----

    /// The picker's two sources: `dashboard` is personalised and carries
    /// `user:onyourwave`; `list` is the full catalogue and does not. [V]
    pub async fn stations_dashboard(&self) -> Result<Vec<StationView>> {
        let d: StationsDashboard = self.get(&format!("{BASE}/rotor/stations/dashboard")).await?;
        Ok(d.stations.iter().map(|e| StationView::from(&e.station)).collect())
    }

    pub async fn stations_list(&self) -> Result<Vec<StationView>> {
        let l: Vec<StationEntry> = self.get(&format!("{BASE}/rotor/stations/list")).await?;
        Ok(l.iter().map(|e| StationView::from(&e.station)).collect())
    }

    pub async fn station_tracks(
        &self,
        station: &str,
        queue: Option<&str>,
    ) -> Result<(Option<String>, Vec<TrackView>)> {
        // Official clients send settings2 on the first fetch and `queue` alone on
        // continuations; the reference library overwrites rather than merges.
        let url = match queue {
            Some(q) => format!("{BASE}/rotor/station/{station}/tracks?queue={q}"),
            None => format!("{BASE}/rotor/station/{station}/tracks?settings2=True"),
        };
        let res: StationTracks = self.get(&url).await?;
        let tracks = res
            .sequence
            .iter()
            .filter_map(|s| s.track.as_ref())
            .map(TrackView::from)
            .collect();
        Ok((res.batch_id, tracks))
    }

    /// Rotor feedback. The wave only adapts if these are sent, so failures are
    /// logged by the caller rather than aborting playback.
    pub async fn station_feedback(
        &self,
        station: &str,
        kind: &str,
        track_id: Option<&str>,
        batch_id: Option<&str>,
        played_seconds: Option<f64>,
    ) -> Result<()> {
        let mut url = format!("{BASE}/rotor/station/{station}/feedback");
        if let Some(b) = batch_id {
            url.push_str(&format!("?batch-id={b}"));
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        // Verified empirically: this endpoint 400s ("condition is not met") on
        // form-encoded bodies and only accepts JSON.
        let mut payload = serde_json::json!({
            "type": kind,
            "timestamp": timestamp,
            "from": "riflu",
        });
        let obj = payload.as_object_mut().expect("json object");
        if let Some(t) = track_id {
            obj.insert("trackId".into(), serde_json::json!(t));
        }
        if let Some(s) = played_seconds {
            obj.insert("totalPlayedSeconds".into(), serde_json::json!(s));
        }

        let _: serde_json::Value = self
            .send(reqwest::Method::POST, &url, Some(&Body::Json(payload)))
            .await?;
        Ok(())
    }

    pub async fn like_track(&self, uid: &str, track_id: &str, like: bool) -> Result<()> {
        let action = if like { "add-multiple" } else { "remove" };
        let body = Body::Form(vec![("track-ids".into(), track_id.to_string())]);
        let _: serde_json::Value = self
            .send(
                reqwest::Method::POST,
                &format!("{BASE}/users/{uid}/likes/tracks/{action}"),
                Some(&body),
            )
            .await?;
        Ok(())
    }
}

/// Via `Value`, which keeps the last of a duplicated key: `/search` repeats
/// `albums` inside every track, and derived structs reject that.
fn decode<T: DeserializeOwned>(url: &str, body: &str) -> Result<T> {
    let bad = |e: serde_json::Error| Error::Api(format!("unexpected response from {url}: {e}"));
    let value: serde_json::Value = serde_json::from_str(body).map_err(bad)?;
    Ok(serde_json::from_value::<Envelope<T>>(value).map_err(bad)?.result)
}

fn truncate(s: &str) -> String {
    if s.len() > 300 { format!("{}…", &s[..300]) } else { s.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_tolerates_duplicate_keys() {
        let body = r#"{"result":{"tracks":{"results":[
            {"id":1,"title":"Qwe","albums":[{"title":"A"}],"albums":[{"title":"A"}]}
        ]}}}"#;
        let res: SearchResult = decode("test", body).unwrap();
        assert_eq!(res.tracks.unwrap().results[0].albums[0].title.as_deref(), Some("A"));
    }
}
