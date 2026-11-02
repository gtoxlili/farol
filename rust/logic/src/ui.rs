//! The setters the logic drives the scene with.
//!
//! GSP setters are asynchronous: `Ok` means GSP accepted the command, which it
//! applies before the next frame; it is not a rendered frame. On the board they
//! are safe from any task and never wait for rendering.

use gsp_sys as gsp;

/// Component key from the generated `GSP_OBJ_KEY_*` constants.
pub type Key = gsp::gsp_component_key_t;
/// Property key from the generated `GSP_PROP_KEY_*` constants.
pub type Property = gsp::gsp_property_key_t;
/// Bind id from the generated `GSP_BIND_*` constants.
pub type Bind = u16;
/// `Err` carries the GSP error code (`ESP_GSP_ERR_*`).
pub type UiResult = Result<(), gsp::esp_gsp_err_t>;

/// Easing curves GSP applies to a tween.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ease {
    Linear,
    /// Cubic ease-out.
    Out,
    /// Cubic ease-in-out.
    InOut,
}

/// Tween timing. `iterations = 0` repeats until stopped; `alternate` plays
/// every other iteration backwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub duration_ms: u32,
    pub iterations: u16,
    pub alternate: bool,
    pub ease: Ease,
}

impl Timing {
    /// One forward run.
    pub const fn once(duration_ms: u32, ease: Ease) -> Self {
        Self {
            duration_ms,
            iterations: 1,
            alternate: false,
            ease,
        }
    }
}

/// The scene operations the logic uses. Add a method here, implement it in
/// [`GspUi`] over `gsp_sys`, and record it in the tests' mock.
pub trait Ui {
    /// Replaces a bound label's text (`"bind"` in the scene). The text must fit
    /// the GSP text capacity and its glyphs must be in the label's charset.
    fn set_text(&mut self, bind: Bind, text: &str) -> UiResult;

    /// Starts a GSP tween of an integer property of a named component (for
    /// example opacity). GSP runs the motion; a new tween on the same
    /// property replaces the running one.
    fn animate_i32(
        &mut self,
        key: Key,
        property: Property,
        from: i32,
        to: i32,
        timing: Timing,
    ) -> UiResult;
}

#[cfg(not(test))]
pub use gsp_ui::GspUi;

#[cfg(not(test))]
mod gsp_ui {
    use super::*;
    use crate::text::TEXT_CAPACITY;

    /// [`Ui`] over a running GSP instance's C API.
    #[derive(Clone, Copy)]
    pub struct GspUi(gsp::esp_gsp_handle_t);

    // The handle is an opaque pointer whose setters GSP documents as any-task safe.
    unsafe impl Send for GspUi {}

    impl GspUi {
        /// # Safety
        /// `handle` must be a started GSP instance that outlives this value.
        pub unsafe fn new(handle: gsp::esp_gsp_handle_t) -> Self {
            Self(handle)
        }

        pub fn handle(self) -> gsp::esp_gsp_handle_t {
            self.0
        }
    }

    fn check(err: gsp::esp_gsp_err_t) -> UiResult {
        if err == gsp::ESP_GSP_OK as gsp::esp_gsp_err_t {
            Ok(())
        } else {
            Err(err)
        }
    }

    fn i32_value(v: i32) -> gsp::gsp_value_t {
        let mut value = gsp::gsp_value_t {
            type_: gsp::GSP_VALUE_I32,
            ..Default::default()
        };
        value.data.i32_ = v;
        value
    }

    fn ease(ease: Ease) -> gsp::esp_gsp_ease_t {
        match ease {
            Ease::Linear => gsp::ESP_GSP_EASE_LINEAR,
            Ease::Out => gsp::ESP_GSP_EASE_OUT,
            Ease::InOut => gsp::ESP_GSP_EASE_IN_OUT,
        }
    }

    impl Ui for GspUi {
        fn set_text(&mut self, bind: Bind, text: &str) -> UiResult {
            // GSP copies the string before returning; the NUL-terminated copy
            // here only needs to live for the call.
            let mut c_text = [0u8; TEXT_CAPACITY + 1];
            if text.len() > TEXT_CAPACITY || text.as_bytes().contains(&0) {
                return Err(gsp::ESP_GSP_ERR_INVALID_ARG as gsp::esp_gsp_err_t);
            }
            c_text[..text.len()].copy_from_slice(text.as_bytes());
            check(unsafe { gsp::esp_gsp_set_text(self.0, bind, c_text.as_ptr().cast()) })
        }

        fn animate_i32(
            &mut self,
            key: Key,
            property: Property,
            from: i32,
            to: i32,
            timing: Timing,
        ) -> UiResult {
            let (from, to) = (i32_value(from), i32_value(to));
            let config = gsp::esp_gsp_animation_config_t {
                duration_ms: timing.duration_ms,
                delay_ms: 0,
                iterations: timing.iterations,
                alternate: timing.alternate,
                ease: ease(timing.ease),
            };
            check(unsafe {
                gsp::esp_gsp_component_play_animation(self.0, key, property, &from, &to, &config)
            })
        }
    }
}
