//! Fixed-capacity UTF-8 text for GSP labels, formatted without allocation.

use core::fmt;

/// Longest text a GSP text setter accepts, in bytes, without the NUL.
pub const TEXT_CAPACITY: usize = gsp_sys::ESP_GSP_BUILD_CAP_TEXT_CAPACITY as usize;

/// A label's text; writes past [`TEXT_CAPACITY`] fail instead of truncating.
pub struct Text {
    bytes: [u8; TEXT_CAPACITY],
    len: usize,
}

impl Text {
    pub const fn new() -> Self {
        Self {
            bytes: [0; TEXT_CAPACITY],
            len: 0,
        }
    }

    pub fn as_str(&self) -> &str {
        // Only whole `&str`s are ever copied in.
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl fmt::Write for Text {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        if end > TEXT_CAPACITY {
            return Err(fmt::Error);
        }
        self.bytes[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
