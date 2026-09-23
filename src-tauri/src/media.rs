//! Stream URL resolution for `download-info`: MP3 only, signed with an MD5 over
//! a fixed salt. Kept free of I/O so the signing is testable.
//! See ANALYSIS.md §5.

use md5::{Digest, Md5};

use crate::error::{Error, Result};
use crate::model::DownloadInfo;

const SIGN_SALT: &str = "XGRlBW9FXlekgbPrRHuSiA";

/// Highest-bitrate non-preview variant. A preview is the 30s clip served to
/// accounts without Plus, so we only fall back to it if nothing else exists.
pub fn pick_best(infos: &[DownloadInfo]) -> Option<&DownloadInfo> {
    let best_full = infos
        .iter()
        .filter(|i| !i.preview.unwrap_or(false))
        .max_by_key(|i| i.bitrate_in_kbps.unwrap_or(0));
    best_full.or_else(|| infos.iter().max_by_key(|i| i.bitrate_in_kbps.unwrap_or(0)))
}

fn text_of(doc: &roxmltree::Document, tag: &str) -> Option<String> {
    doc.descendants()
        .find(|n| n.has_tag_name(tag))
        .and_then(|n| n.text())
        .map(str::to_owned)
}

/// Turn the download-info XML into a playable URL.
pub fn build_direct_link(xml: &str) -> Result<String> {
    let doc = roxmltree::Document::parse(xml)?;
    let missing = |f: &str| Error::Api(format!("download-info xml missing <{f}>"));

    let host = text_of(&doc, "host").ok_or_else(|| missing("host"))?;
    let path = text_of(&doc, "path").ok_or_else(|| missing("path"))?;
    let ts = text_of(&doc, "ts").ok_or_else(|| missing("ts"))?;
    let s = text_of(&doc, "s").ok_or_else(|| missing("s"))?;

    // The leading slash of <path> is excluded from the signed string but kept in the URL.
    let path_body = path.strip_prefix('/').unwrap_or(&path);
    let mut hasher = Md5::new();
    hasher.update(format!("{SIGN_SALT}{path_body}{s}").as_bytes());
    let sign = hex::encode(hasher.finalize());

    Ok(format!("https://{host}/get-mp3/{sign}/{ts}{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_link_and_excludes_leading_slash_from_signature() {
        let xml = r#"<?xml version="1.0"?><download-info>
            <host>s1.example.net</host><path>/abc/def.mp3</path>
            <ts>0123456789abcdef</ts><s>deadbeef</s></download-info>"#;

        let url = build_direct_link(xml).unwrap();

        let mut h = Md5::new();
        h.update(format!("{SIGN_SALT}abc/def.mp3deadbeef").as_bytes());
        let expected = hex::encode(h.finalize());

        assert_eq!(url, format!("https://s1.example.net/get-mp3/{expected}/0123456789abcdef/abc/def.mp3"));
    }

    #[test]
    fn prefers_full_track_over_preview() {
        let infos = vec![
            DownloadInfo { codec: "mp3".into(), bitrate_in_kbps: Some(320), preview: Some(true), download_info_url: "a".into() },
            DownloadInfo { codec: "mp3".into(), bitrate_in_kbps: Some(192), preview: Some(false), download_info_url: "b".into() },
        ];
        assert_eq!(pick_best(&infos).unwrap().download_info_url, "b");
    }
}
