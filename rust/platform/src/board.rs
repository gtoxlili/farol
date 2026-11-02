//! The board's side of [`logic::sys::Sys`]: the build flavour, ESP-IDF
//! storage, identity and diagnostics.
//!
//! Prototypes: `app_logic.h` (this repo), `nvs.h`, `esp_mac.h`, `esp_random.h`,
//! `esp_heap_caps.h`, FreeRTOS `task.h` (pinned IDF 7b9cc1ac).
//!
//! Saved state lives in the application's `nvs` partition, namespace
//! [`NAMESPACE`]: `prefs` (fixed bytes) and `paseo` ([`Config::encode`]: the
//! direct connection or the relay pairing, and the daemon password). The
//! password is stored as the user typed it; NVS is not encrypted on this board.

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{CStr, c_char, c_int, c_void};

use logic::lang::Lang;
use logic::paseo::Config;
use logic::sys::Prefs;

use crate::idf;

type EspErr = c_int;
const ESP_OK: EspErr = 0;
/// `ESP_ERR_NVS_NOT_FOUND`.
const ESP_ERR_NVS_NOT_FOUND: EspErr = 0x1102;
/// `NVS_READONLY` / `NVS_READWRITE`.
const NVS_READONLY: c_int = 0;
const NVS_READWRITE: c_int = 1;
/// `MALLOC_CAP_INTERNAL`.
const MALLOC_CAP_INTERNAL: u32 = 1 << 11;

const NAMESPACE: &CStr = c"farol";
const KEY_PREFS: &CStr = c"prefs";
const KEY_PASEO: &CStr = c"paseo";
/// Leading byte of the prefs record; bump when its layout changes (2 added
/// the language).
const PREFS_VERSION: u8 = 2;
/// A saved connection and password never exceed this.
const PASEO_RECORD_MAX: usize = 1024;

unsafe extern "C" {
    fn app_logic_demo_build() -> bool;
    fn esp_fill_random(buf: *mut c_void, len: usize);

    fn nvs_open(namespace: *const c_char, mode: c_int, handle: *mut u32) -> EspErr;
    fn nvs_get_blob(
        handle: u32,
        key: *const c_char,
        out: *mut c_void,
        length: *mut usize,
    ) -> EspErr;
    fn nvs_set_blob(handle: u32, key: *const c_char, value: *const c_void, length: usize)
    -> EspErr;
    fn nvs_erase_key(handle: u32, key: *const c_char) -> EspErr;
    fn nvs_commit(handle: u32) -> EspErr;
    fn nvs_close(handle: u32);

    fn esp_efuse_mac_get_default(mac: *mut u8) -> EspErr;
    fn heap_caps_get_free_size(caps: u32) -> usize;
    fn heap_caps_get_minimum_free_size(caps: u32) -> usize;
    fn uxTaskGetStackHighWaterMark(task: *mut c_void) -> u32;
}

fn warn(what: &str, err: EspErr) {
    idf::log_line(
        idf::LogLevel::Warn,
        c"board",
        &alloc::format!("{what} failed (0x{err:x})"),
    );
}

// ---------------------------------------------------------------- build

/// A development build that runs the demo fleet (`CONFIG_FAROL_DEMO`).
pub fn demo_build() -> bool {
    unsafe { app_logic_demo_build() }
}

// ---------------------------------------------------------------- identity

/// Four hex digits from the factory MAC: short, stable, and enough to tell
/// a few boards apart in Paseo ("farol-3fa2").
pub fn device_id() -> String {
    let mut mac = [0u8; 6];
    if unsafe { esp_efuse_mac_get_default(mac.as_mut_ptr()) } != ESP_OK {
        return String::from("0000");
    }
    alloc::format!("{:02x}{:02x}", mac[4], mac[5])
}

// ---------------------------------------------------------------- storage

struct Nvs(u32);

impl Nvs {
    fn open(write: bool) -> Option<Self> {
        let mut handle = 0;
        let mode = if write { NVS_READWRITE } else { NVS_READONLY };
        let err = unsafe { nvs_open(NAMESPACE.as_ptr(), mode, &mut handle) };
        // A fresh partition has no namespace until the first write.
        if err != ESP_OK {
            if write || err != ESP_ERR_NVS_NOT_FOUND {
                warn("nvs open", err);
            }
            return None;
        }
        Some(Self(handle))
    }

    fn get(&self, key: &CStr, max: usize) -> Option<Vec<u8>> {
        let mut length = 0usize;
        let err = unsafe { nvs_get_blob(self.0, key.as_ptr(), core::ptr::null_mut(), &mut length) };
        if err != ESP_OK || length > max {
            if err != ESP_ERR_NVS_NOT_FOUND && err != ESP_OK {
                warn("nvs read", err);
            }
            return None;
        }
        let mut data = alloc::vec![0u8; length];
        let err =
            unsafe { nvs_get_blob(self.0, key.as_ptr(), data.as_mut_ptr().cast(), &mut length) };
        (err == ESP_OK).then(|| {
            data.truncate(length);
            data
        })
    }

    fn put(&self, key: &CStr, value: Option<&[u8]>) {
        let err = match value {
            Some(bytes) => unsafe {
                nvs_set_blob(self.0, key.as_ptr(), bytes.as_ptr().cast(), bytes.len())
            },
            None => match unsafe { nvs_erase_key(self.0, key.as_ptr()) } {
                ESP_ERR_NVS_NOT_FOUND => ESP_OK,
                err => err,
            },
        };
        let err = if err == ESP_OK {
            unsafe { nvs_commit(self.0) }
        } else {
            err
        };
        if err != ESP_OK {
            warn("nvs write", err);
        }
    }
}

impl Drop for Nvs {
    fn drop(&mut self) {
        unsafe { nvs_close(self.0) }
    }
}

fn encode_prefs(p: &Prefs) -> [u8; 6] {
    [
        PREFS_VERSION,
        u8::from(p.sound),
        u8::from(p.haptic),
        p.brightness,
        u8::from(p.auto),
        u8::from(p.lang == Lang::En),
    ]
}

/// Version 1 had no language: it reads as Chinese.
fn decode_prefs(bytes: &[u8]) -> Option<Prefs> {
    let (fields, lang) = match bytes {
        [1, rest @ ..] if rest.len() == 4 => (rest, Lang::Zh),
        [PREFS_VERSION, rest @ .., lang] if rest.len() == 4 => {
            (rest, if *lang == 1 { Lang::En } else { Lang::Zh })
        }
        _ => return None,
    };
    let [sound, haptic, brightness, auto] = fields else {
        return None;
    };
    Some(Prefs {
        sound: *sound != 0,
        haptic: *haptic != 0,
        brightness: (*brightness).clamp(5, 100),
        auto: *auto != 0,
        lang,
    })
}

/// Bytes from the hardware RNG. With Wi-Fi running (the relay needs it) they
/// are true random numbers, as ESP-IDF documents for `esp_fill_random`.
pub fn random(out: &mut [u8]) {
    // SAFETY: `out` is a valid, writable buffer of `out.len()` bytes.
    unsafe { esp_fill_random(out.as_mut_ptr().cast(), out.len()) }
}

/// What was saved, for [`logic::sys::Event::Restored`]; defaults when nothing was.
pub fn load() -> (Option<Config>, Prefs) {
    let Some(nvs) = Nvs::open(false) else {
        return (None, Prefs::default());
    };
    let prefs = nvs
        .get(KEY_PREFS, 16)
        .as_deref()
        .and_then(decode_prefs)
        .unwrap_or_default();
    let config = nvs
        .get(KEY_PASEO, PASEO_RECORD_MAX)
        .as_deref()
        .and_then(Config::decode);
    (config, prefs)
}

pub fn save_prefs(prefs: &Prefs) {
    if let Some(nvs) = Nvs::open(true) {
        nvs.put(KEY_PREFS, Some(&encode_prefs(prefs)));
    }
}

pub fn save_config(config: Option<&Config>) {
    if let Some(nvs) = Nvs::open(true) {
        nvs.put(KEY_PASEO, config.map(Config::encode).as_deref());
    }
}

// ---------------------------------------------------------------- diagnostics

/// One line for the device log: the calling task's unused stack and the heaps.
pub fn health_line() -> String {
    let (stack, internal, internal_min, psram) = unsafe {
        (
            uxTaskGetStackHighWaterMark(core::ptr::null_mut()),
            heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
            heap_caps_get_minimum_free_size(MALLOC_CAP_INTERNAL),
            heap_caps_get_free_size(idf::MALLOC_CAP_SPIRAM),
        )
    };
    alloc::format!(
        "stack_unused={stack} B internal_free={} KiB (min {} KiB) psram_free={} KiB",
        internal / 1024,
        internal_min / 1024,
        psram / 1024
    )
}
