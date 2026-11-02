// SPDX-License-Identifier: Apache-2.0
/* Frame probes. Linker wraps (CMakeLists.txt) observe, without changing:
 *  - esp_display_presenter_begin_next_frame: GSP starts drawing a frame;
 *  - esp_display_presenter_commit_frame_with_receipt: GSP finished drawing and
 *    submits (GSP 1.9 commits only through this call; the receipt names the frame);
 *  - esp_display_present_tracker_complete_present_frame_isr: the presenter's
 *    TE compose pool retires a frame when its panel transfer is done (ISR).
 * GSP's own counters (esp_gsp_debug.h) are cumulative only, so per-frame
 * timing comes from these points.
 *
 * Producers: the GSP render task (touch samples, begin, commit) and the
 * panel transfer ISR (done). Consumer: the bench task. Each path is a
 * single-producer/single-consumer ring. Touch samples and the frame being
 * built are touched by GSP's render task only; a spinlock still guards them
 * in case the adapter polls touch from another task. */

#include <stdatomic.h>
#include <string.h>

#include "bench_internal.h"
#include "esp_attr.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"

#define RING_SIZE 64u   /* power of two; >1 s of frames at 60 fps */
#define MATCH_SLOTS 32u /* frames in flight between commit and done */

/* --- render-task state ------------------------------------------------- */

static portMUX_TYPE s_render_lock = portMUX_INITIALIZER_UNLOCKED;
static struct {
    int64_t pending_us[BENCH_FRAME_SAMPLES_MAX];
    uint8_t pending_count;
    uint8_t claimed_at_begin;  /* samples delivered before the frame began */
    int64_t begin_us;
    bool building;
} s_render;

void bench_probe_touch_sample(int64_t sample_us)
{
    portENTER_CRITICAL(&s_render_lock);
    if (s_render.pending_count < BENCH_FRAME_SAMPLES_MAX) {
        s_render.pending_us[s_render.pending_count++] = sample_us;
    }
    portEXIT_CRITICAL(&s_render_lock);
}

/* --- rings -------------------------------------------------------------- */

typedef struct {
    uint64_t id;
    int64_t done_us;
} done_t;

/* Harness buffers stay out of internal RAM so the mem line reflects the app;
 * only the ISR-written ring must be internal. */
static EXT_RAM_BSS_ATTR struct {
    bench_frame_t items[RING_SIZE];
    atomic_uint head, tail;
} s_commits;

static DRAM_ATTR struct {
    done_t items[RING_SIZE];
    atomic_uint head, tail;
} s_dones;

static esp_display_presenter_t *_Atomic s_presenter;

void bench_probe_attach(esp_display_presenter_t *presenter)
{
    atomic_store(&s_presenter, presenter);
}

/* --- wraps -------------------------------------------------------------- */

esp_err_t __real_esp_display_presenter_begin_next_frame(
    esp_display_presenter_t *presenter, const esp_display_present_surface_request_t *request,
    esp_display_present_area_t *out_render_areas, size_t render_area_capacity,
    size_t *out_render_area_count, bool *out_full_coverage);

esp_err_t __wrap_esp_display_presenter_begin_next_frame(
    esp_display_presenter_t *presenter, const esp_display_present_surface_request_t *request,
    esp_display_present_area_t *out_render_areas, size_t render_area_capacity,
    size_t *out_render_area_count, bool *out_full_coverage)
{
    const esp_err_t err = __real_esp_display_presenter_begin_next_frame(
                              presenter, request, out_render_areas, render_area_capacity,
                              out_render_area_count, out_full_coverage);
    if (err == ESP_OK) {
        const int64_t begin_us = esp_timer_get_time();
        portENTER_CRITICAL(&s_render_lock);
        s_render.begin_us = begin_us;
        s_render.claimed_at_begin = s_render.pending_count;
        s_render.building = true;
        portEXIT_CRITICAL(&s_render_lock);
    }
    return err;
}

esp_err_t __real_esp_display_presenter_commit_frame_with_receipt(
    esp_display_presenter_t *presenter, const esp_display_presenter_submit_t *submit,
    bool track_display, esp_display_presenter_frame_receipt_t *out_receipt);

esp_err_t __wrap_esp_display_presenter_commit_frame_with_receipt(
    esp_display_presenter_t *presenter, const esp_display_presenter_submit_t *submit,
    bool track_display, esp_display_presenter_frame_receipt_t *out_receipt)
{
    const int64_t commit_us = esp_timer_get_time();
    const esp_err_t err = __real_esp_display_presenter_commit_frame_with_receipt(
                              presenter, submit, track_display, out_receipt);
    const bool ours = err == ESP_OK && out_receipt != NULL && out_receipt->frame_id != 0 &&
                      presenter == atomic_load(&s_presenter);

    bench_frame_t frame = {0};
    portENTER_CRITICAL(&s_render_lock);
    const bool record = ours && s_render.building;
    const uint8_t claimed = s_render.claimed_at_begin;
    if (record) {
        frame = (bench_frame_t) {
            .id = out_receipt->frame_id,
            .begin_us = s_render.begin_us,
            .commit_us = commit_us,
            .sample_count = claimed,
        };
        memcpy(frame.samples_us, s_render.pending_us, claimed * sizeof(int64_t));
        /* Samples that arrived while this frame was drawn wait for the next. */
        memmove(s_render.pending_us, s_render.pending_us + claimed,
                (s_render.pending_count - claimed) * sizeof(int64_t));
        s_render.pending_count -= claimed;
    }
    s_render.claimed_at_begin = 0;
    s_render.building = false;
    portEXIT_CRITICAL(&s_render_lock);

    if (record) {
        const unsigned head = atomic_load_explicit(&s_commits.head, memory_order_relaxed);
        const unsigned tail = atomic_load_explicit(&s_commits.tail, memory_order_acquire);
        if (head - tail < RING_SIZE) {
            s_commits.items[head % RING_SIZE] = frame;
            atomic_store_explicit(&s_commits.head, head + 1, memory_order_release);
        }
    }
    return err;
}

bool __real_esp_display_present_tracker_complete_present_frame_isr(void *tracker, uint64_t frame_id);

bool IRAM_ATTR __wrap_esp_display_present_tracker_complete_present_frame_isr(
    void *tracker, uint64_t frame_id)
{
    const bool completed =
        __real_esp_display_present_tracker_complete_present_frame_isr(tracker, frame_id);
    if (completed) {
        const unsigned head = atomic_load_explicit(&s_dones.head, memory_order_relaxed);
        const unsigned tail = atomic_load_explicit(&s_dones.tail, memory_order_acquire);
        if (head - tail < RING_SIZE) {
            s_dones.items[head % RING_SIZE] = (done_t) {
                frame_id, esp_timer_get_time()
            };
            atomic_store_explicit(&s_dones.head, head + 1, memory_order_release);
        }
    }
    return completed;
}

/* --- bench task: pair commits with completions -------------------------- */

static EXT_RAM_BSS_ATTR struct {
    bench_frame_t commit[MATCH_SLOTS];
    done_t done[MATCH_SLOTS];
    bench_frame_t ready[RING_SIZE];
    unsigned ready_head, ready_tail;
} s_match;

static void emit(const bench_frame_t *commit, int64_t done_us)
{
    if (s_match.ready_head - s_match.ready_tail >= RING_SIZE) {
        return;
    }
    bench_frame_t *frame = &s_match.ready[s_match.ready_head++ % RING_SIZE];
    *frame = *commit;
    frame->done_us = done_us;
}

static void drain(void)
{
    unsigned tail = atomic_load_explicit(&s_commits.tail, memory_order_relaxed);
    const unsigned commit_head = atomic_load_explicit(&s_commits.head, memory_order_acquire);
    for (; tail != commit_head; ++tail) {
        const bench_frame_t *commit = &s_commits.items[tail % RING_SIZE];
        done_t *done = &s_match.done[commit->id % MATCH_SLOTS];
        if (done->id == commit->id && commit->id != 0) {
            emit(commit, done->done_us);
            done->id = 0;
        } else {
            s_match.commit[commit->id % MATCH_SLOTS] = *commit;
        }
    }
    atomic_store_explicit(&s_commits.tail, tail, memory_order_release);

    tail = atomic_load_explicit(&s_dones.tail, memory_order_relaxed);
    const unsigned done_head = atomic_load_explicit(&s_dones.head, memory_order_acquire);
    for (; tail != done_head; ++tail) {
        const done_t done = s_dones.items[tail % RING_SIZE];
        bench_frame_t *commit = &s_match.commit[done.id % MATCH_SLOTS];
        if (commit->id == done.id && done.id != 0) {
            emit(commit, done.done_us);
            commit->id = 0;
        } else {
            s_match.done[done.id % MATCH_SLOTS] = done;
        }
    }
    atomic_store_explicit(&s_dones.tail, tail, memory_order_release);
}

bool bench_probe_pop(bench_frame_t *out_frame)
{
    if (s_match.ready_head == s_match.ready_tail) {
        drain();
    }
    if (s_match.ready_head == s_match.ready_tail) {
        return false;
    }
    *out_frame = s_match.ready[s_match.ready_tail++ % RING_SIZE];
    return true;
}
