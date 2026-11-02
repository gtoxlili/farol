//! Paseo's dictation, as the board speaks it on its daemon session (Paseo
//! 0.14, `packages/protocol/src/messages.ts`, the `dictation_stream_*`
//! messages). The board streams 16 kHz mono 16-bit PCM in base64 chunks; the
//! daemon's speech-to-text (configured in Paseo) answers with the transcript.
//!
//! Order: `dictation_stream_start`, then chunks numbered from 0 once the
//! daemon acks the start (`ackSeq: -1`; earlier audio waits here), then
//! `dictation_stream_finish` with the last number, or `dictation_stream_cancel`.
//! The daemon answers `dictation_stream_partial`, `dictation_stream_final` or
//! `dictation_stream_error`, all tied to the stream's `dictationId`.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde_json::{Value, json};

use crate::relay;

/// The only format Paseo takes: raw PCM, signed 16-bit little-endian, mono.
pub const FORMAT: &str = "audio/pcm;rate=16000;bits=16";
pub const SAMPLE_RATE: u32 = 16_000;
/// After `finish`, the daemon's final transcript is due within its stated
/// timeout; this is used when it states none.
const FINAL_WAIT_MS: u64 = 15_000;
/// Margin over the daemon's own finish timeout.
const FINAL_MARGIN_MS: u64 = 5_000;

/// What the compose page learns about the current dictation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// The transcript so far.
    Partial(String),
    /// The whole transcript; the stream is over.
    Final(String),
    /// The stream failed; the reason is Paseo's (or the board's) message.
    Failed(String),
}

/// One dictation stream.
pub struct Dictation {
    id: String,
    /// The daemon acknowledged the start; chunks may flow.
    ready: bool,
    next_seq: u64,
    /// Encoded chunks waiting for the start's ack.
    queued: Vec<String>,
    /// The user let go: finish once everything queued went out.
    finishing: bool,
    /// When the final transcript is overdue.
    due: Option<u64>,
}

impl Dictation {
    /// Starts stream `id`; returns it and the start frame.
    pub fn start(id: &str) -> (Self, String) {
        let frame = session(json!({
            "type": "dictation_stream_start",
            "dictationId": id,
            "format": FORMAT,
        }));
        let me = Self {
            id: id.to_string(),
            ready: false,
            next_seq: 0,
            queued: Vec::new(),
            finishing: false,
            due: None,
        };
        (me, frame)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// No audio went out or waits to.
    pub fn is_empty(&self) -> bool {
        self.next_seq == 0 && self.queued.is_empty()
    }

    /// Some PCM: the frames to send now (none until the daemon is ready).
    pub fn audio(&mut self, pcm: &[u8]) -> Vec<String> {
        if self.finishing || pcm.is_empty() {
            return Vec::new();
        }
        let chunk = relay::encode(pcm);
        if !self.ready {
            self.queued.push(chunk);
            return Vec::new();
        }
        alloc::vec![self.chunk(chunk)]
    }

    fn chunk(&mut self, audio: String) -> String {
        let seq = self.next_seq;
        self.next_seq += 1;
        session(json!({
            "type": "dictation_stream_chunk",
            "dictationId": self.id,
            "seq": seq,
            "audio": audio,
            "format": FORMAT,
        }))
    }

    /// The user let go: the frames that end the stream (later, if the daemon
    /// has not acknowledged the start yet). An empty recording cancels.
    pub fn finish(&mut self, now: u64) -> Vec<String> {
        self.finishing = true;
        self.due = Some(now + FINAL_WAIT_MS + FINAL_MARGIN_MS);
        if !self.ready {
            return Vec::new();
        }
        self.finish_frames()
    }

    fn finish_frames(&mut self) -> Vec<String> {
        if self.next_seq == 0 {
            // Paseo refuses a finish without audio.
            return alloc::vec![self.cancel()];
        }
        alloc::vec![session(json!({
            "type": "dictation_stream_finish",
            "dictationId": self.id,
            "finalSeq": self.next_seq - 1,
        }))]
    }

    /// The frame that drops the stream.
    pub fn cancel(&self) -> String {
        session(json!({"type": "dictation_stream_cancel", "dictationId": self.id}))
    }

    /// A session message about this stream: frames to send, and what was
    /// heard. `None` for messages about other streams.
    pub fn on_message(
        &mut self,
        now: u64,
        message: &Value,
    ) -> Option<(Vec<String>, Option<Heard>)> {
        if message["payload"]["dictationId"].as_str() != Some(self.id.as_str()) {
            return None;
        }
        let payload = &message["payload"];
        let text = || payload["text"].as_str().unwrap_or("").trim().to_string();
        let mut out = Vec::new();
        let heard = match message["type"].as_str()? {
            "dictation_stream_ack" if !self.ready => {
                self.ready = true;
                for chunk in core::mem::take(&mut self.queued) {
                    let frame = self.chunk(chunk);
                    out.push(frame);
                }
                if self.finishing {
                    out.extend(self.finish_frames());
                }
                None
            }
            "dictation_stream_finish_accepted" => {
                let wait = payload["timeoutMs"].as_u64().unwrap_or(FINAL_WAIT_MS);
                self.due = Some(now + wait + FINAL_MARGIN_MS);
                None
            }
            "dictation_stream_partial" => Some(Heard::Partial(text())),
            "dictation_stream_final" => Some(Heard::Final(text())),
            "dictation_stream_error" => Some(Heard::Failed(
                payload["error"]
                    .as_str()
                    .unwrap_or("dictation failed")
                    .to_string(),
            )),
            _ => None,
        };
        Some((out, heard))
    }

    /// When the final transcript is overdue, if the user let go.
    pub fn due(&self) -> Option<u64> {
        self.due
    }
}

fn session(message: Value) -> String {
    json!({"type": "session", "message": message}).to_string()
}

/// A dictation id unique on this board: `farol-<device>-<n>`.
pub fn id(device: &str, n: u32) -> String {
    format!("farol-{device}-{n}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(text: &str) -> Value {
        serde_json::from_str::<Value>(text).unwrap()["message"].clone()
    }

    fn reply(kind: &str, payload: Value) -> Value {
        json!({"type": kind, "payload": payload})
    }

    #[test]
    fn audio_waits_for_the_start_ack_then_flows_in_order() {
        let (mut d, start) = Dictation::start("farol-3a1f-1");
        let start = frame(&start);
        assert_eq!(start["type"], "dictation_stream_start");
        assert_eq!(start["format"], FORMAT);
        assert!(d.audio(&[1, 0, 2, 0]).is_empty());
        let ack = reply(
            "dictation_stream_ack",
            json!({"dictationId": "farol-3a1f-1", "ackSeq": -1}),
        );
        let (frames, heard) = d.on_message(0, &ack).unwrap();
        assert_eq!(heard, None);
        let chunk = frame(&frames[0]);
        assert_eq!(chunk["seq"], 0);
        assert_eq!(chunk["audio"], "AQACAA==");
        assert_eq!(chunk["format"], FORMAT);
        assert_eq!(frame(&d.audio(&[3, 0])[0])["seq"], 1);
        let finish = frame(&d.finish(10)[0]);
        assert_eq!(finish["type"], "dictation_stream_finish");
        assert_eq!(finish["finalSeq"], 1);
        let done = reply(
            "dictation_stream_final",
            json!({"dictationId": "farol-3a1f-1", "text": " 修一下登录页 "}),
        );
        assert_eq!(
            d.on_message(20, &done).unwrap().1,
            Some(Heard::Final("修一下登录页".into()))
        );
    }

    #[test]
    fn letting_go_before_the_ack_finishes_after_it() {
        let (mut d, _) = Dictation::start("x");
        d.audio(&[1, 0]);
        assert!(d.finish(5).is_empty());
        let ack = reply(
            "dictation_stream_ack",
            json!({"dictationId": "x", "ackSeq": -1}),
        );
        let frames: Vec<Value> = d
            .on_message(6, &ack)
            .unwrap()
            .0
            .iter()
            .map(|f| frame(f))
            .collect();
        assert_eq!(frames[0]["type"], "dictation_stream_chunk");
        assert_eq!(frames[1]["type"], "dictation_stream_finish");
        assert_eq!(frames[1]["finalSeq"], 0);
    }

    #[test]
    fn an_empty_recording_cancels_and_other_streams_are_not_ours() {
        let (mut d, _) = Dictation::start("x");
        let ack = reply(
            "dictation_stream_ack",
            json!({"dictationId": "x", "ackSeq": -1}),
        );
        d.on_message(0, &ack);
        assert_eq!(frame(&d.finish(1)[0])["type"], "dictation_stream_cancel");
        let other = reply(
            "dictation_stream_final",
            json!({"dictationId": "y", "text": "hi"}),
        );
        assert!(d.on_message(2, &other).is_none());
        let error = reply(
            "dictation_stream_error",
            json!({"dictationId": "x", "error": "Dictation STT not configured"}),
        );
        assert_eq!(
            d.on_message(3, &error).unwrap().1,
            Some(Heard::Failed("Dictation STT not configured".into()))
        );
    }
}
