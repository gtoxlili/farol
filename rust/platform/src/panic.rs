//! Panic handler: report the message through ESP-IDF logging, then `abort()`.
//!
//! `abort()` enters ESP-IDF's panic path (backtrace, core dump to the
//! retained `coredump` partition, reboot), so Rust panics are diagnosed like C
//! aborts. The message is formatted into a static buffer: the heap may be the
//! reason for the panic and task stacks are small.

use core::cell::UnsafeCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::idf;

const PANIC_MESSAGE_CAPACITY: usize = 256;

struct PanicBuffer(UnsafeCell<[u8; PANIC_MESSAGE_CAPACITY]>);

// Only the first panicking task obtains the buffer (guarded by PANICKING).
unsafe impl Sync for PanicBuffer {}

static PANIC_BUFFER: PanicBuffer = PanicBuffer(UnsafeCell::new([0; PANIC_MESSAGE_CAPACITY]));
static PANICKING: AtomicBool = AtomicBool::new(false);

struct Truncating<'a> {
    buffer: &'a mut [u8],
    used: usize,
}

impl Write for Truncating<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let room = self.buffer.len() - self.used;
        let mut take = text.len().min(room);
        while !text.is_char_boundary(take) {
            take -= 1;
        }
        self.buffer[self.used..self.used + take].copy_from_slice(&text.as_bytes()[..take]);
        self.used += take;
        Ok(())
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    if !PANICKING.swap(true, Ordering::AcqRel) {
        // SAFETY: the swap above grants exclusive access to the buffer.
        let buffer = unsafe { &mut *PANIC_BUFFER.0.get() };
        let mut writer = Truncating { buffer, used: 0 };
        let _ = write!(writer, "Rust panic: {info}");
        let used = writer.used;
        // SAFETY: Truncating only copies whole UTF-8 characters.
        let text = unsafe { core::str::from_utf8_unchecked(&buffer[..used]) };
        idf::log_line(idf::LogLevel::Error, c"logic", text);
    }
    unsafe { idf::abort() }
}
