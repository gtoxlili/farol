//! Global allocator backed by the ESP-IDF capability heap.
//!
//! Rust data never needs internal RAM (no DMA, no ISR access), so every
//! allocation and every growth prefers PSRAM and falls back to any
//! `MALLOC_CAP_DEFAULT` heap; internal RAM stays free for DMA, Wi-Fi and task
//! stacks. Asking for `MALLOC_CAP_DEFAULT` alone would not do this: on
//! ESP32-S31 internal RAM matches first at every size
//! (heap/port/esp32s31/memory_layout.c), for `realloc` as well.
//!
//! ESP-IDF's TLSF heap only guarantees 4-byte alignment (`ALIGN_SIZE_LOG2 = 2`
//! in `heap/tlsf/tlsf_control_functions.h`), while Rust types such as `u64`
//! and `f64` need 8 on riscv32. Larger alignments therefore go through
//! `heap_caps_aligned_alloc`; `heap_caps_free` releases both kinds.

use core::alloc::{GlobalAlloc, Layout};

use crate::idf;

/// Alignment that `heap_caps_malloc` guarantees for every allocation.
const HEAP_MIN_ALIGN: usize = 4;
const CAPS_ANY: u32 = idf::MALLOC_CAP_DEFAULT;
const CAPS_PSRAM: u32 = idf::MALLOC_CAP_DEFAULT | idf::MALLOC_CAP_SPIRAM;

struct IdfHeap;

#[global_allocator]
static HEAP: IdfHeap = IdfHeap;

/// Allocates with the preferred capabilities, then from any default heap.
unsafe fn allocate(layout: Layout, caps: u32) -> *mut u8 {
    unsafe {
        if layout.align() <= HEAP_MIN_ALIGN {
            idf::heap_caps_malloc(layout.size(), caps).cast()
        } else {
            idf::heap_caps_aligned_alloc(layout.align(), layout.size(), caps).cast()
        }
    }
}

unsafe impl GlobalAlloc for IdfHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { allocate(layout, CAPS_PSRAM) };
        if !ptr.is_null() {
            return ptr;
        }
        unsafe { allocate(layout, CAPS_ANY) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        unsafe { idf::heap_caps_free(ptr.cast()) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if layout.align() <= HEAP_MIN_ALIGN {
            // On failure the block is left as it was, so the fallback can retry it.
            let grown = unsafe { idf::heap_caps_realloc(ptr.cast(), new_size, CAPS_PSRAM) };
            if !grown.is_null() {
                return grown.cast();
            }
            return unsafe { idf::heap_caps_realloc(ptr.cast(), new_size, CAPS_ANY) }.cast();
        }
        // heap_caps_realloc does not preserve over-alignment: allocate, copy, free.
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        let new_ptr = unsafe { self.alloc(new_layout) };
        if !new_ptr.is_null() {
            unsafe {
                core::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        new_ptr
    }
}
