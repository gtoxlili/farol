//! The application's state machine.

use core::fmt::Write;

use gsp_sys as gsp;

use crate::text::Text;
use crate::ui::{Ease, Timing, Ui, UiResult};

/// Inputs the platforms deliver to [`App::handle`]. Plain data, so the board
/// can pass it through a FreeRTOS queue by copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Event {
    /// The "点一下" button was tapped (scene callback `tap`).
    Tap,
}

impl Event {
    /// Decodes a GSP event; `None` for events the logic ignores. Pure, so the
    /// platforms call it inside the GSP callback before handing the result on.
    pub fn from_gsp(event: &gsp::esp_gsp_event_t) -> Option<Self> {
        if event.type_ != gsp::ESP_GSP_EVENT_CALL {
            return None;
        }
        match u32::from(event.action_id) {
            gsp::GSP_ACT_ID_TAP => Some(Self::Tap),
            _ => None,
        }
    }
}

/// The uptime label changes once a second, on a grid from start.
const UPTIME_PERIOD_MS: u64 = 1000;
/// The tap marker fades in on the first tap and flashes on later ones.
const PULSE_HIDDEN: i32 = 0;
const PULSE_DIM: i32 = 96;
const PULSE_FULL: i32 = 255;
const PULSE_TIMING: Timing = Timing::once(300, Ease::Out);

pub struct App {
    taps: u32,
    started_ms: u64,
    next_uptime_ms: u64,
    rejections: u32,
    first_rejection: Option<gsp::esp_gsp_err_t>,
    rejection_reported: bool,
}

impl App {
    /// `now_ms` is the platform's monotonic clock; all later times use it.
    pub fn new(now_ms: u64) -> Self {
        Self {
            taps: 0,
            started_ms: now_ms,
            next_uptime_ms: now_ms,
            rejections: 0,
            first_rejection: None,
            rejection_reported: false,
        }
    }

    pub fn handle(&mut self, event: Event, ui: &mut impl Ui) {
        match event {
            Event::Tap => self.tap(ui),
        }
    }

    /// Does the work that is due at `now_ms`. Calling it early or repeatedly is
    /// harmless.
    pub fn poll(&mut self, now_ms: u64, ui: &mut impl Ui) {
        if now_ms >= self.next_uptime_ms {
            let seconds = now_ms.saturating_sub(self.started_ms) / UPTIME_PERIOD_MS;
            self.show_uptime(seconds, ui);
            // Stay on the one-second grid; skip missed seconds instead of bursting.
            self.next_uptime_ms = self.started_ms + (seconds + 1) * UPTIME_PERIOD_MS;
        }
    }

    /// When [`App::poll`] is next due, on the same clock as `now_ms`.
    pub fn next_deadline_ms(&self) -> u64 {
        self.next_uptime_ms
    }

    pub fn taps(&self) -> u32 {
        self.taps
    }

    /// Setter calls GSP rejected so far.
    pub fn rejections(&self) -> u32 {
        self.rejections
    }

    /// The first rejected setter's GSP error code, returned once so the
    /// platform logs it once; later rejections only count.
    pub fn take_first_rejection(&mut self) -> Option<gsp::esp_gsp_err_t> {
        if self.rejection_reported {
            return None;
        }
        let first = self.first_rejection;
        self.rejection_reported = first.is_some();
        first
    }

    fn tap(&mut self, ui: &mut impl Ui) {
        self.taps = self.taps.saturating_add(1);
        let mut status = Text::new();
        // "已点击 N 次": every glyph is in the status label's font_charset.
        if write!(status, "已点击 {} 次", self.taps).is_ok() {
            let result = ui.set_text(gsp::GSP_BIND_STATUS as u16, status.as_str());
            self.note(result);
        }
        let from = if self.taps == 1 {
            PULSE_HIDDEN
        } else {
            PULSE_DIM
        };
        let result = ui.animate_i32(
            gsp::GSP_OBJ_KEY_PULSE,
            gsp::GSP_PROP_KEY_OPACITY,
            from,
            PULSE_FULL,
            PULSE_TIMING,
        );
        self.note(result);
    }

    fn show_uptime(&mut self, seconds: u64, ui: &mut impl Ui) {
        let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
        let mut text = Text::new();
        if write!(text, "{h:02}:{m:02}:{s:02}").is_ok() {
            let result = ui.set_text(gsp::GSP_BIND_UPTIME as u16, text.as_str());
            self.note(result);
        }
    }

    fn note(&mut self, result: UiResult) {
        if let Err(code) = result {
            self.rejections = self.rejections.saturating_add(1);
            self.first_rejection.get_or_insert(code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{Bind, Key, Property};

    #[derive(Debug, PartialEq)]
    enum Call {
        Text(Bind, String),
        Animate(Key, Property, i32, i32, Timing),
    }

    #[derive(Default)]
    struct Recorder {
        calls: Vec<Call>,
        reject: Option<gsp::esp_gsp_err_t>,
    }

    impl Recorder {
        fn result(&self) -> UiResult {
            self.reject.map_or(Ok(()), Err)
        }
    }

    impl Ui for Recorder {
        fn set_text(&mut self, bind: Bind, text: &str) -> UiResult {
            self.calls.push(Call::Text(bind, text.to_string()));
            self.result()
        }

        fn animate_i32(
            &mut self,
            key: Key,
            property: Property,
            from: i32,
            to: i32,
            timing: Timing,
        ) -> UiResult {
            self.calls
                .push(Call::Animate(key, property, from, to, timing));
            self.result()
        }
    }

    const STATUS: Bind = gsp::GSP_BIND_STATUS as Bind;
    const UPTIME: Bind = gsp::GSP_BIND_UPTIME as Bind;

    fn call_event(action_id: u32) -> gsp::esp_gsp_event_t {
        gsp::esp_gsp_event_t {
            type_: gsp::ESP_GSP_EVENT_CALL,
            action_id: action_id as u16,
            ..Default::default()
        }
    }

    #[test]
    fn decodes_only_the_tap_callback() {
        assert_eq!(
            Event::from_gsp(&call_event(gsp::GSP_ACT_ID_TAP)),
            Some(Event::Tap)
        );
        assert_eq!(Event::from_gsp(&call_event(gsp::GSP_ACT_ID_TAP + 1)), None);
        let scene_changed = gsp::esp_gsp_event_t {
            type_: gsp::ESP_GSP_EVENT_SCENE_CHANGED,
            action_id: gsp::GSP_ACT_ID_TAP as u16,
            ..Default::default()
        };
        assert_eq!(Event::from_gsp(&scene_changed), None);
    }

    #[test]
    fn first_tap_counts_and_fades_the_marker_in() {
        let mut app = App::new(0);
        let mut ui = Recorder::default();
        app.handle(Event::Tap, &mut ui);
        assert_eq!(
            ui.calls,
            [
                Call::Text(STATUS, "已点击 1 次".into()),
                Call::Animate(
                    gsp::GSP_OBJ_KEY_PULSE,
                    gsp::GSP_PROP_KEY_OPACITY,
                    PULSE_HIDDEN,
                    PULSE_FULL,
                    PULSE_TIMING
                ),
            ]
        );
    }

    #[test]
    fn later_taps_flash_the_marker_from_dim() {
        let mut app = App::new(0);
        let mut ui = Recorder::default();
        app.handle(Event::Tap, &mut ui);
        app.handle(Event::Tap, &mut ui);
        assert_eq!(app.taps(), 2);
        assert_eq!(ui.calls[2], Call::Text(STATUS, "已点击 2 次".into()));
        assert!(matches!(
            ui.calls[3],
            Call::Animate(_, _, PULSE_DIM, PULSE_FULL, _)
        ));
    }

    #[test]
    fn uptime_follows_a_one_second_grid() {
        let mut app = App::new(5_000);
        let mut ui = Recorder::default();
        app.poll(5_000, &mut ui);
        assert_eq!(app.next_deadline_ms(), 6_000);
        app.poll(5_999, &mut ui);
        // A late poll shows the current second and skips the missed ones.
        app.poll(5_000 + 3_725_400, &mut ui);
        assert_eq!(app.next_deadline_ms(), 5_000 + 3_726_000);
        assert_eq!(
            ui.calls,
            [
                Call::Text(UPTIME, "00:00:00".into()),
                Call::Text(UPTIME, "01:02:05".into()),
            ]
        );
    }

    #[test]
    fn rejections_are_counted_and_the_first_reported_once() {
        let mut app = App::new(0);
        let mut ui = Recorder {
            reject: Some(gsp::ESP_GSP_ERR_INVALID_STATE as gsp::esp_gsp_err_t),
            ..Default::default()
        };
        app.handle(Event::Tap, &mut ui);
        assert_eq!(app.rejections(), 2);
        assert_eq!(
            app.take_first_rejection(),
            Some(gsp::ESP_GSP_ERR_INVALID_STATE as gsp::esp_gsp_err_t)
        );
        assert_eq!(app.take_first_rejection(), None);
    }
}
