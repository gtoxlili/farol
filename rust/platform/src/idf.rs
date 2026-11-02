//! Hand-written ESP-IDF / FreeRTOS C surface used by this crate (pinned IDF
//! 7b9cc1ac). Prototypes: `esp_heap_caps.h`, `esp_log_write.h`,
//! `esp_log_timestamp.h`, `esp_timer.h`, `stdlib.h`, and
//! FreeRTOS `queue.h` / `idf_additions.h` (BaseType_t = int,
//! UBaseType_t = unsigned int, TickType_t = uint32_t, stack depth in bytes).

use core::ffi::{c_char, c_int, c_void};

/// `esp_log_level_t` values from `esp_log_level.h`.
#[allow(dead_code)]
#[repr(u32)]
#[derive(Clone, Copy)]
pub enum LogLevel {
    Error = 1,
    Warn = 2,
    Info = 3,
}

/// `MALLOC_CAP_DEFAULT` from `esp_heap_caps.h`: memory that plain `malloc()` may return.
pub const MALLOC_CAP_DEFAULT: u32 = 1 << 12;
/// `MALLOC_CAP_SPIRAM` from `esp_heap_caps.h`: external PSRAM.
pub const MALLOC_CAP_SPIRAM: u32 = 1 << 10;

/// `queueQUEUE_TYPE_BASE` and `queueSEND_TO_BACK` from `queue.h`.
const QUEUE_TYPE_BASE: u8 = 0;
const SEND_TO_BACK: c_int = 0;
/// `pdPASS` / `pdTRUE`.
const PD_PASS: c_int = 1;
/// `tskNO_AFFINITY` on ESP-IDF.
const NO_AFFINITY: c_int = 0x7FFF_FFFF;

/// FreeRTOS tick period. `components/app_logic/app_logic.c` asserts
/// `CONFIG_FREERTOS_HZ == 1000` at C compile time, so one tick is one millisecond.
pub const TICK_MS: u32 = 1;

pub type TaskFn = unsafe extern "C" fn(*mut c_void);

unsafe extern "C" {
    pub fn heap_caps_malloc(size: usize, caps: u32) -> *mut c_void;
    pub fn heap_caps_aligned_alloc(alignment: usize, size: usize, caps: u32) -> *mut c_void;
    pub fn heap_caps_realloc(ptr: *mut c_void, size: usize, caps: u32) -> *mut c_void;
    pub fn heap_caps_free(ptr: *mut c_void);

    fn esp_log_timestamp() -> u32;
    fn esp_log_write(level: LogLevel, tag: *const c_char, format: *const c_char, ...);

    fn esp_timer_get_time() -> i64;
    pub fn abort() -> !;

    fn xQueueGenericCreate(length: u32, item_size: u32, queue_type: u8) -> *mut c_void;
    fn xQueueGenericSend(
        queue: *mut c_void,
        item: *const c_void,
        ticks: u32,
        position: c_int,
    ) -> c_int;
    fn xQueueReceive(queue: *mut c_void, item: *mut c_void, ticks: u32) -> c_int;
    fn xTaskCreatePinnedToCore(
        task: TaskFn,
        name: *const c_char,
        stack_bytes: u32,
        arg: *mut c_void,
        priority: u32,
        handle: *mut *mut c_void,
        core: c_int,
    ) -> c_int;
}

/// A FreeRTOS queue of `T` values copied by bytes. `T` must be plain data.
pub struct Queue<T: Copy> {
    handle: *mut c_void,
    _item: core::marker::PhantomData<T>,
}

// FreeRTOS queues are designed for cross-task (and render-task) use.
unsafe impl<T: Copy> Send for Queue<T> {}
unsafe impl<T: Copy> Sync for Queue<T> {}

impl<T: Copy> Queue<T> {
    pub fn new(length: u32) -> Option<Self> {
        let size = u32::try_from(core::mem::size_of::<T>()).ok()?;
        let handle = unsafe { xQueueGenericCreate(length, size, QUEUE_TYPE_BASE) };
        (!handle.is_null()).then_some(Self {
            handle,
            _item: core::marker::PhantomData,
        })
    }

    /// Never blocks; false when the queue is full.
    pub fn try_send(&self, item: T) -> bool {
        unsafe {
            xQueueGenericSend(self.handle, (&raw const item).cast(), 0, SEND_TO_BACK) == PD_PASS
        }
    }

    /// Waits up to `timeout_ms` for one item.
    pub fn recv(&self, timeout_ms: u32) -> Option<T> {
        let mut slot = core::mem::MaybeUninit::<T>::uninit();
        let ok =
            unsafe { xQueueReceive(self.handle, slot.as_mut_ptr().cast(), timeout_ms / TICK_MS) };
        (ok == PD_PASS).then(|| unsafe { slot.assume_init() })
    }
}

/// Starts a FreeRTOS task with the stack in internal RAM, on either core.
pub fn spawn(
    name: &'static core::ffi::CStr,
    stack_bytes: u32,
    priority: u32,
    f: TaskFn,
    arg: *mut c_void,
) -> bool {
    let mut handle = core::ptr::null_mut();
    unsafe {
        xTaskCreatePinnedToCore(
            f,
            name.as_ptr(),
            stack_bytes,
            arg,
            priority,
            &mut handle,
            NO_AFFINITY,
        ) == PD_PASS
    }
}

/// Milliseconds since boot (esp_timer, monotonic).
pub fn now_ms() -> u64 {
    // esp_timer counts up from zero at boot and never goes negative.
    (unsafe { esp_timer_get_time() } / 1000) as u64
}

/// Writes one complete log line through ESP-IDF logging (same layout as `ESP_LOGx`).
pub fn log_line(level: LogLevel, tag: &core::ffi::CStr, message: &str) {
    let letter = match level {
        LogLevel::Error => b'E',
        LogLevel::Warn => b'W',
        LogLevel::Info => b'I',
    };
    let length = c_int::try_from(message.len()).unwrap_or(c_int::MAX);
    unsafe {
        esp_log_write(
            level,
            tag.as_ptr(),
            c"%c (%lu) %s: %.*s\n".as_ptr(),
            c_int::from(letter),
            esp_log_timestamp(),
            tag.as_ptr(),
            length,
            message.as_ptr(),
        );
    }
}
