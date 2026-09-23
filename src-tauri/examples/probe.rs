//! End-to-end API check without the UI: `cargo run --example probe`.
//! Signs in (or reuses the stored token), then verifies account, wave,
//! feedback, and that a resolved stream URL serves bytes.

use yamusic_lib::{api::Api, auth, store};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let http = reqwest::Client::new();

    let token = match store::load()? {
        Some(t) => {
            println!("✓ reusing stored token");
            t
        }
        None => {
            let code = auth::request_device_code(&http, "yamusic-probe").await?;
            println!("\n  Open {}", code.verification_url);
            println!("  Enter code: {}\n", code.user_code);
            println!("  waiting for confirmation…");

            let deadline = std::time::Instant::now()
                + std::time::Duration::from_secs(code.expires_in.min(600));
            loop {
                if std::time::Instant::now() > deadline {
                    return Err("sign-in timed out".into());
                }
                tokio::time::sleep(std::time::Duration::from_secs(code.interval.max(1))).await;
                if let Some(t) = auth::poll_device_token(&http, &code.device_code).await? {
                    store::save(&t)?;
                    println!("✓ signed in, token saved");
                    break t;
                }
            }
        }
    };

    let api = Api::new(token);

    let account = api.account_status().await?;
    println!("✓ account: {} (uid {})", account.display_name, account.uid);
    println!("  Plus: {}", if account.has_plus { "yes" } else { "NO — expect 30s previews" });

    // Pull one track from Моя волна.
    let (batch_id, tracks) = api.station_tracks("user:onyourwave", None).await?;
    println!("✓ wave batch {:?}: {} tracks", batch_id, tracks.len());
    let track = tracks.first().ok_or("wave returned no tracks")?;
    println!("  first: {} — {}", track.artist, track.title);

    // Feedback only validates once a batch exists, so it is exercised in order.
    api.station_feedback("user:onyourwave", "radioStarted", None, batch_id.as_deref(), None)
        .await?;
    println!("✓ rotor accepted radioStarted");
    api.station_feedback("user:onyourwave", "trackStarted", Some(&track.id), batch_id.as_deref(), None)
        .await?;
    println!("✓ rotor accepted trackStarted");

    let url = api.track_url(&track.id).await?;
    println!("✓ resolved stream URL");

    // The load-bearing check: does the signed URL actually serve audio?
    // Some stream hosts reject HEAD, so fall back to a one-byte ranged GET.
    let mut resp = api.http().head(&url).send().await?;
    let mut how = "HEAD";
    if !resp.status().is_success() {
        resp = api.http().get(&url).header("Range", "bytes=0-0").send().await?;
        how = "GET Range";
    }
    println!(
        "✓ {} {} | content-type: {:?} | length: {:?} | accept-ranges: {:?}",
        how,
        resp.status(),
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()),
        resp.headers().get("content-length").and_then(|v| v.to_str().ok()),
        resp.headers().get("accept-ranges").and_then(|v| v.to_str().ok()),
    );

    if !resp.status().is_success() {
        return Err(format!("stream URL not playable: {}", resp.status()).into());
    }
    // Like, then immediately un-like, so the account is left as it was.
    api.like_track(&account.uid, &track.id, true).await?;
    api.like_track(&account.uid, &track.id, false).await?;
    println!("✓ like/un-like round-trip (library unchanged)");

    println!("\nAll checks passed.");
    Ok(())
}
