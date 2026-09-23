//! "Моя волна" is a stateful server-side session, not a playlist: each batch is
//! computed from the feedback we sent for the previous one. This owns that
//! session so the UI only ever asks for "the next track".

use std::collections::VecDeque;

use crate::api::Api;
use crate::error::Result;
use crate::model::TrackView;

/// Refill this many tracks before the queue runs dry, so there is no gap.
const PREFETCH_THRESHOLD: usize = 2;

pub const MY_WAVE: &str = "user:onyourwave";

#[derive(Default)]
pub struct WaveSession {
    pub station: String,
    pub batch_id: Option<String>,
    queue: VecDeque<TrackView>,
    /// Id of the last track handed out — the rotor uses it to continue the wave.
    last_track_id: Option<String>,
    started: bool,
}

impl WaveSession {
    pub fn new(station: &str) -> Self {
        Self { station: station.to_string(), ..Default::default() }
    }

    pub fn batch_id(&self) -> Option<&str> {
        self.batch_id.as_deref()
    }

    /// Tracks buffered ahead of the one now playing.
    pub fn upcoming(&self) -> Vec<TrackView> {
        self.queue.iter().cloned().collect()
    }

    /// Drop queued tracks to jump forward. No feedback is sent: these were
    /// never started, so nothing was skipped in the rotor's sense.
    pub fn drop_ahead(&mut self, n: usize) {
        for _ in 0..n.min(self.queue.len()) {
            self.queue.pop_front();
        }
    }

    /// Begin the wave. Order matters: the rotor rejects `radioStarted` unless it
    /// carries the `batchId` of an already-fetched batch.
    pub async fn start(&mut self, api: &Api) -> Result<()> {
        self.fetch_batch(api).await?;
        if !self.started {
            api.station_feedback(
                &self.station,
                "radioStarted",
                None,
                self.batch_id.as_deref(),
                None,
            )
            .await?;
            self.started = true;
        }
        Ok(())
    }

    async fn fetch_batch(&mut self, api: &Api) -> Result<()> {
        let (batch_id, tracks) = api
            .station_tracks(&self.station, self.last_track_id.as_deref())
            .await?;
        self.batch_id = batch_id;
        self.queue.extend(tracks.into_iter().filter(|t| t.available));
        Ok(())
    }

    /// Hand out the next track, topping the queue up when it runs low.
    pub async fn next_track(&mut self, api: &Api) -> Result<Option<TrackView>> {
        if !self.started {
            self.start(api).await?;
        }
        if self.queue.len() <= PREFETCH_THRESHOLD {
            // A failed refill should not break playback while tracks remain.
            if let Err(e) = self.fetch_batch(api).await {
                if self.queue.is_empty() {
                    return Err(e);
                }
                eprintln!("wave refill failed, playing from buffer: {e}");
            }
        }
        let track = self.queue.pop_front();
        if let Some(t) = &track {
            self.last_track_id = Some(t.id.clone());
        }
        Ok(track)
    }

    /// Drop buffered tracks so settings changes take effect immediately.
    pub fn reset(&mut self) {
        self.queue.clear();
        self.batch_id = None;
        self.last_track_id = None;
        self.started = false;
    }
}
