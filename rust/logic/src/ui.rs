//! The scene operations the logic uses, and [`Painter`], which drops writes
//! that would not change what is on screen.
//!
//! GSP setters are asynchronous: `Ok` means GSP accepted the command, which it
//! applies before the next frame. On the board they are safe from any task and
//! never wait for rendering. Colors are 0xRRGGBB; [`GspUi`] converts them to
//! the scene's RGB565.

use alloc::collections::BTreeMap;
use alloc::string::String;

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

/// Every scene operation the logic performs. [`GspUi`] implements it over the
/// GSP C API; the tests record the calls instead.
pub trait Ui {
    /// Replaces a bound label's text. Glyphs outside the baked set come from
    /// the dynamic (FreeType) font.
    fn set_text(&mut self, bind: Bind, text: &str) -> UiResult;
    /// Shows or hides an object with a visibility bind, with its subtree.
    fn set_visible(&mut self, bind: Bind, visible: bool) -> UiResult;
    /// Sets the color of an object with a color bind.
    fn set_color(&mut self, bind: Bind, rgb: u32) -> UiResult;
    /// Tweens the color of an object with a color bind.
    fn animate_color(&mut self, bind: Bind, from: u32, to: u32, ms: u32, ease: Ease) -> UiResult;
    /// Sets a value control (arc, progress, orb level, slider).
    fn set_value(&mut self, key: Key, value: i32) -> UiResult;
    /// Tweens a value control.
    fn animate_value(&mut self, key: Key, from: i32, to: i32, ms: u32, ease: Ease) -> UiResult;
    /// Sets an integer runtime property (`x`, `y`, an effect's `period_ms`...).
    fn set_i32(&mut self, key: Key, property: Property, value: i32) -> UiResult;
    /// Sets a color runtime property such as an effect's `fg_color`.
    fn set_property_color(&mut self, key: Key, property: Property, rgb: u32) -> UiResult;
    /// Tweens an integer runtime property (an effect's `phase`...).
    fn animate_i32(
        &mut self,
        key: Key,
        property: Property,
        from: i32,
        to: i32,
        ms: u32,
        ease: Ease,
    ) -> UiResult;
    /// Routes the scene's keyboard (`action` is its `GSP_ACT_ID_*_KEY`) into
    /// the text bind `bind`, starting from empty text. Only its OK key reaches
    /// the logic, as a `Call` with `action`.
    fn keyboard_attach(&mut self, action: u16, bind: Bind) -> UiResult;
    /// The attached keyboard's current text.
    fn keyboard_text(&mut self) -> Result<String, gsp::esp_gsp_err_t>;
    /// Selects a PageFlow page.
    fn set_page(&mut self, key: Key, page: u16, animated: bool) -> UiResult;
    /// Opens or closes a Drawer.
    fn set_drawer(&mut self, key: Key, open: bool, animated: bool) -> UiResult;
}

/// What a [`Painter`] last wrote to one target, to skip identical writes.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Written {
    Text(u64),
    Flag(bool),
    Number(i32),
}

/// Wraps a [`Ui`] and forgets nothing: identical repeated writes are dropped,
/// so the views can describe the whole screen on every update without
/// flooding GSP. Tweens always pass through and invalidate the cached value.
/// The first rejected setter is kept for one log line. A rejected write is
/// not cached, so the next render retries it.
pub struct Painter {
    written: BTreeMap<(u8, u32), Written>,
    rejections: u32,
    first_rejection: Option<Rejection>,
    /// Some write failed since the last [`Painter::take_failed`].
    failed: bool,
}

/// A setter GSP refused, for the log: its code, the kind of target and the
/// target's bind id or component key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rejection {
    pub code: gsp::esp_gsp_err_t,
    pub what: &'static str,
    pub target: u32,
}

const TEXT: u8 = 0;
const VISIBLE: u8 = 1;
const COLOR: u8 = 2;
const VALUE: u8 = 3;
const PROPERTY: u8 = 4;
const PAGE: u8 = 5;

impl Default for Painter {
    fn default() -> Self {
        Self::new()
    }
}

impl Painter {
    pub const fn new() -> Self {
        Self {
            written: BTreeMap::new(),
            rejections: 0,
            first_rejection: None,
            failed: false,
        }
    }

    /// Forget everything written, e.g. after the scene was reset.
    pub fn forget(&mut self) {
        self.written.clear();
    }

    pub fn rejections(&self) -> u32 {
        self.rejections
    }

    pub fn take_first_rejection(&mut self) -> Option<Rejection> {
        self.first_rejection.take()
    }

    /// Whether a write failed since the last call (and will be retried).
    pub fn take_failed(&mut self) -> bool {
        core::mem::take(&mut self.failed)
    }

    fn write(&mut self, slot: (u8, u32), value: Written, apply: impl FnOnce() -> UiResult) {
        if self.written.get(&slot) == Some(&value) {
            return;
        }
        match apply() {
            Ok(()) => {
                self.written.insert(slot, value);
            }
            Err(code) => self.reject(code, slot),
        }
    }

    fn reject(&mut self, code: gsp::esp_gsp_err_t, (kind, target): (u8, u32)) {
        self.rejections = self.rejections.saturating_add(1);
        self.failed = true;
        let what = match kind {
            TEXT => "text bind",
            VISIBLE => "visibility bind",
            COLOR => "color bind",
            VALUE => "value of key",
            PROPERTY => "property slot",
            PAGE => "page of key",
            _ => "call",
        };
        self.first_rejection
            .get_or_insert(Rejection { code, what, target });
    }

    fn result(&mut self, result: UiResult) {
        if let Err(code) = result {
            self.reject(code, (u8::MAX, 0));
        }
    }

    /// Records that the scene already shows `text` (its default), so the
    /// first render does not write it again.
    pub fn assume_text(&mut self, bind: Bind, text: &str) {
        self.written
            .insert((TEXT, bind.into()), Written::Text(hash(text)));
    }

    pub fn text(&mut self, ui: &mut impl Ui, bind: Bind, text: &str) {
        self.write((TEXT, bind.into()), Written::Text(hash(text)), || {
            ui.set_text(bind, text)
        });
    }

    pub fn visible(&mut self, ui: &mut impl Ui, bind: Bind, visible: bool) {
        self.write((VISIBLE, bind.into()), Written::Flag(visible), || {
            ui.set_visible(bind, visible)
        });
    }

    pub fn color(&mut self, ui: &mut impl Ui, bind: Bind, rgb: u32) {
        self.write((COLOR, bind.into()), Written::Number(rgb as i32), || {
            ui.set_color(bind, rgb)
        });
    }

    /// Tweens from the color last written (or `rgb` if none) to `rgb`.
    pub fn fade_color(&mut self, ui: &mut impl Ui, bind: Bind, rgb: u32, ms: u32) {
        let slot = (COLOR, u32::from(bind));
        let from = match self.written.get(&slot) {
            Some(Written::Number(n)) => *n as u32,
            _ => rgb,
        };
        if from == rgb && self.written.contains_key(&slot) {
            return;
        }
        let result = ui.animate_color(bind, from, rgb, ms, Ease::Out);
        if result.is_ok() {
            self.written.insert(slot, Written::Number(rgb as i32));
        }
        self.result(result);
    }

    pub fn value(&mut self, ui: &mut impl Ui, key: Key, value: i32) {
        self.write((VALUE, key), Written::Number(value), || {
            ui.set_value(key, value)
        });
    }

    /// Tweens to `value` from what was last written; the end value is cached.
    pub fn glide_value(&mut self, ui: &mut impl Ui, key: Key, value: i32, ms: u32) {
        let slot = (VALUE, key);
        match self.written.get(&slot) {
            Some(Written::Number(from)) if *from == value => {}
            Some(Written::Number(from)) => {
                let from = *from;
                let result = ui.animate_value(key, from, value, ms, Ease::Out);
                if result.is_ok() {
                    self.written.insert(slot, Written::Number(value));
                }
                self.result(result);
            }
            _ => self.value(ui, key, value),
        }
    }

    pub fn property(&mut self, ui: &mut impl Ui, key: Key, property: Property, value: i32) {
        let slot = (PROPERTY, key ^ property.rotate_left(7));
        self.write(slot, Written::Number(value), || {
            ui.set_i32(key, property, value)
        });
    }

    pub fn property_color(&mut self, ui: &mut impl Ui, key: Key, property: Property, rgb: u32) {
        let slot = (PROPERTY, key ^ property.rotate_left(7));
        self.write(slot, Written::Number(rgb as i32), || {
            ui.set_property_color(key, property, rgb)
        });
    }

    /// Selects a page; `None` cached means "unknown", so the write goes out.
    pub fn page(&mut self, ui: &mut impl Ui, key: Key, page: u16, animated: bool) {
        self.write((PAGE, key), Written::Number(page.into()), || {
            ui.set_page(key, page, animated)
        });
    }

    /// Records a page the user reached by swiping, so a later write of the
    /// same page is not sent again.
    pub fn note_page(&mut self, key: Key, page: u16) {
        self.written
            .insert((PAGE, key), Written::Number(page.into()));
    }

    /// One-off operations that have no cached state.
    pub fn run(&mut self, result: UiResult) {
        self.result(result);
    }
}

/// FNV-1a; collisions only cost a skipped write of identical-hash text.
fn hash(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// 0xRRGGBB to the scene's RGB565.
pub const fn rgb565(rgb: u32) -> u32 {
    let r = (rgb >> 16) & 0xFF;
    let g = (rgb >> 8) & 0xFF;
    let b = rgb & 0xFF;
    ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
}

#[cfg(not(test))]
pub use gsp_ui::GspUi;

#[cfg(not(test))]
mod gsp_ui {
    use alloc::vec::Vec;

    use super::*;

    /// Longest label text accepted, in bytes; longer text is cut at a char
    /// boundary. GSP copies text of any length, this only bounds the copy.
    const MAX_TEXT: usize = 2048;
    /// A daemon address or a password never needs more.
    const KEYBOARD_MAX_BYTES: usize = 256;

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

    fn ease(ease: Ease) -> gsp::esp_gsp_ease_t {
        match ease {
            Ease::Linear => gsp::ESP_GSP_EASE_LINEAR,
            Ease::Out => gsp::ESP_GSP_EASE_OUT,
            Ease::InOut => gsp::ESP_GSP_EASE_IN_OUT,
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

    impl Ui for GspUi {
        fn set_text(&mut self, bind: Bind, text: &str) -> UiResult {
            let mut end = text.len().min(MAX_TEXT);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let mut c_text: Vec<u8> = Vec::with_capacity(end + 1);
            c_text.extend(
                text.as_bytes()[..end]
                    .iter()
                    .map(|&b| if b == 0 { b' ' } else { b }),
            );
            c_text.push(0);
            check(unsafe { gsp::esp_gsp_set_text(self.0, bind, c_text.as_ptr().cast()) })
        }

        fn set_visible(&mut self, bind: Bind, visible: bool) -> UiResult {
            check(unsafe { gsp::esp_gsp_set_visible(self.0, bind, visible) })
        }

        fn set_color(&mut self, bind: Bind, rgb: u32) -> UiResult {
            check(unsafe { gsp::esp_gsp_set_color(self.0, bind, rgb565(rgb)) })
        }

        fn animate_color(&mut self, bind: Bind, from: u32, to: u32, ms: u32, e: Ease) -> UiResult {
            check(unsafe {
                gsp::esp_gsp_animate_color(self.0, bind, rgb565(from), rgb565(to), ms, ease(e))
            })
        }

        fn set_value(&mut self, key: Key, value: i32) -> UiResult {
            check(unsafe { gsp::esp_gsp_component_set_value(self.0, key, value) })
        }

        fn animate_value(&mut self, key: Key, from: i32, to: i32, ms: u32, e: Ease) -> UiResult {
            check(unsafe {
                gsp::esp_gsp_component_animate_value(self.0, key, from, to, ms, ease(e))
            })
        }

        fn set_i32(&mut self, key: Key, property: Property, value: i32) -> UiResult {
            let value = i32_value(value);
            check(unsafe { gsp::esp_gsp_component_set_property(self.0, key, property, &value) })
        }

        fn set_property_color(&mut self, key: Key, property: Property, rgb: u32) -> UiResult {
            check(unsafe {
                gsp::esp_gsp_component_set_property_color_rgb888(self.0, key, property, rgb)
            })
        }

        fn animate_i32(
            &mut self,
            key: Key,
            property: Property,
            from: i32,
            to: i32,
            ms: u32,
            e: Ease,
        ) -> UiResult {
            let (from, to) = (i32_value(from), i32_value(to));
            check(unsafe {
                gsp::esp_gsp_component_animate_property(
                    self.0,
                    key,
                    property,
                    &from,
                    &to,
                    ms,
                    ease(e),
                )
            })
        }

        fn keyboard_attach(&mut self, action: u16, bind: Bind) -> UiResult {
            check(unsafe {
                gsp::esp_gsp_keyboard_attach_ex(self.0, action, bind, KEYBOARD_MAX_BYTES)
            })
        }

        fn keyboard_text(&mut self) -> Result<String, gsp::esp_gsp_err_t> {
            let mut size = 0usize;
            check(unsafe { gsp::esp_gsp_keyboard_text_size(self.0, &mut size) })?;
            let mut buffer = alloc::vec![0u8; size.max(1)];
            check(unsafe {
                gsp::esp_gsp_keyboard_text(self.0, buffer.as_mut_ptr().cast(), buffer.len())
            })?;
            let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
            Ok(String::from_utf8_lossy(&buffer[..end]).into_owned())
        }

        fn set_page(&mut self, key: Key, page: u16, animated: bool) -> UiResult {
            check(unsafe { gsp::esp_gsp_page_flow_set_page(self.0, key, page, animated) })
        }

        fn set_drawer(&mut self, key: Key, open: bool, animated: bool) -> UiResult {
            check(unsafe {
                if open {
                    gsp::esp_gsp_drawer_open(self.0, key, animated)
                } else {
                    gsp::esp_gsp_drawer_close(self.0, key, animated)
                }
            })
        }
    }
}
