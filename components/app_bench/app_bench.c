// SPDX-License-Identifier: Apache-2.0
/* Bench task: drives three scripted runs and prints one BENCH line per phase
 * plus a memory line per run (README.md). Frames are assigned to the phase in
 * which their presentation completed. */

#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "app_bench.h"
#include "bench_internal.h"
#include "esp_check.h"
#include "esp_gsp_debug.h"
#include "esp_heap_caps.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

static const char *TAG = "bench";

#define BENCH_RUNS 3
#define BENCH_FW "farol"
#define POLL_MS 10
/* A frame completing after its phase ended belongs to the next phase; wait
 * this long past a boundary before closing a phase. */
#define PHASE_GRACE_US 200000
#define FRAMES_MAX 768   /* 5 s idle phase at up to ~150 fps */
#define LATENCY_MAX 512
#define TASK_STACK_BYTES 4096
#define TASK_PRIORITY 2

typedef struct {
    uint32_t count;
    uint32_t values[FRAMES_MAX];
} series_t;

typedef struct {
    uint32_t frames;
    series_t interval, render, present;
    uint32_t latency_count;
    uint32_t latency[LATENCY_MAX];
    /* GSP's own cumulative counters, sampled at the phase boundaries. */
    uint32_t gsp_frames_start;
    uint64_t gsp_render_start, gsp_submit_start;
    uint32_t gsp_frames;
    uint64_t gsp_render_us, gsp_submit_us;
    bool boundary_sampled;
    bool closed;
} phase_stats_t;

/* Owned by the bench task; the phase arrays live in PSRAM. */
typedef struct {
    esp_gsp_handle_t ui;
    esp_display_presenter_t *presenter;
    app_bench_hooks_t hooks;
    phase_stats_t *phases;   /* BENCH_PHASE_COUNT entries */
    int64_t last_done_us;
} bench_t;

static bench_t *s_bench;

static void push(series_t *series, int64_t value)
{
    if (series->count < FRAMES_MAX && value >= 0) {
        series->values[series->count++] = value > UINT32_MAX ? UINT32_MAX : (uint32_t)value;
    }
}

static int compare_u32(const void *a, const void *b)
{
    const uint32_t x = *(const uint32_t *)a, y = *(const uint32_t *)b;
    return (x > y) - (x < y);
}

/* Nearest-rank percentiles; zeros for an empty series. */
static void print_summary(const char *key, uint32_t *values, uint32_t count, bool last)
{
    uint32_t p50 = 0, p95 = 0, max = 0;
    if (count > 0) {
        qsort(values, count, sizeof(values[0]), compare_u32);
        p50 = values[(count * 50 + 99) / 100 - 1];
        p95 = values[(count * 95 + 99) / 100 - 1];
        max = values[count - 1];
    }
    printf("\"%s\":{\"p50\":%" PRIu32 ",\"p95\":%" PRIu32 ",\"max\":%" PRIu32 "}%s",
           key, p50, p95, max, last ? "" : ",");
}

/* Presented frames per second from the transfer-done intervals, in tenths. */
static uint32_t fps_tenths(const series_t *interval)
{
    uint64_t sum_us = 0;
    for (uint32_t i = 0; i < interval->count; ++i) {
        sum_us += interval->values[i];
    }
    return sum_us ? (uint32_t)((uint64_t)interval->count * 10000000u / sum_us) : 0;
}

static void print_phase(int run, bench_phase_t phase, phase_stats_t *stats)
{
    const uint32_t fps = fps_tenths(&stats->interval);
    printf("BENCH {\"fw\":\"" BENCH_FW "\",\"run\":%d,\"phase\":\"%s\",\"frames\":%" PRIu32
           ",\"fps\":%" PRIu32 ".%" PRIu32 ",",
           run, bench_script_phases()[phase].name, stats->frames, fps / 10, fps % 10);
    print_summary("interval_us", stats->interval.values, stats->interval.count, false);
    print_summary("render_us", stats->render.values, stats->render.count, false);
    print_summary("present_us", stats->present.values, stats->present.count, false);
    print_summary("latency_us", stats->latency, stats->latency_count, false);
    printf("\"gsp\":{\"frames\":%" PRIu32 ",\"render_us\":%" PRIu64 ",\"submit_us\":%" PRIu64 "}}\n",
           stats->gsp_frames, stats->gsp_render_us, stats->gsp_submit_us);
    fflush(stdout);
}

static void print_memory(int run)
{
    printf("BENCH {\"fw\":\"" BENCH_FW "\",\"run\":%d,\"mem\":{\"internal_free\":%u,"
           "\"internal_min\":%u,\"psram_free\":%u,\"psram_min\":%u}}\n",
           run,
           (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
           (unsigned)heap_caps_get_minimum_free_size(MALLOC_CAP_INTERNAL),
           (unsigned)heap_caps_get_free_size(MALLOC_CAP_SPIRAM),
           (unsigned)heap_caps_get_minimum_free_size(MALLOC_CAP_SPIRAM));
    fflush(stdout);
}

/* Cumulative GSP counters; phase deltas are taken at boundaries. */
static void gsp_counters(bench_t *bench, uint32_t *frames, uint64_t *render_us, uint64_t *submit_us)
{
    uint64_t busy_us;
    esp_gsp_render_stats(bench->ui, frames, &busy_us);
    esp_gsp_render_phases(bench->ui, render_us, submit_us);
}

static int phase_of(int64_t t_ms)
{
    const bench_phase_window_t *phases = bench_script_phases();
    for (int i = 0; i < BENCH_PHASE_COUNT; ++i) {
        if (t_ms >= phases[i].start_ms && t_ms < phases[i].end_ms) {
            return i;
        }
    }
    return -1;
}

static void account(bench_t *bench, const bench_frame_t *frame, int64_t t0_us)
{
    const int64_t prev_done_us = bench->last_done_us;
    bench->last_done_us = frame->done_us;
    if (frame->done_us < t0_us) {
        return;
    }
    const int phase = phase_of((frame->done_us - t0_us) / 1000);
    if (phase < 0 || bench->phases[phase].closed) {
        return;
    }
    phase_stats_t *stats = &bench->phases[phase];
    stats->frames++;
    if (prev_done_us > 0) {
        push(&stats->interval, frame->done_us - prev_done_us);
    }
    push(&stats->render, frame->commit_us - frame->begin_us);
    push(&stats->present, frame->done_us - frame->commit_us);
    for (uint8_t i = 0; i < frame->sample_count && stats->latency_count < LATENCY_MAX; ++i) {
        stats->latency[stats->latency_count++] = (uint32_t)(frame->done_us - frame->samples_us[i]);
    }
}

static void run_once(bench_t *bench, int run)
{
    memset(bench->phases, 0, sizeof(phase_stats_t) * BENCH_PHASE_COUNT);
    if (bench->hooks.reset_scene && !bench->hooks.reset_scene()) {
        ESP_LOGW(TAG, "scene reset was not accepted");
    }
    const int64_t t0_us = esp_timer_get_time();
    bench_clock_start_run(t0_us);
    const bench_phase_window_t *windows = bench_script_phases();
    gsp_counters(bench, &bench->phases[0].gsp_frames_start, &bench->phases[0].gsp_render_start,
                 &bench->phases[0].gsp_submit_start);
    bench->phases[0].boundary_sampled = true;

    int next_to_close = 0;
    while (next_to_close < BENCH_PHASE_COUNT) {
        vTaskDelay(pdMS_TO_TICKS(POLL_MS));
        const int64_t now_us = esp_timer_get_time();
        /* GSP counters at each boundary: phase k ends where k + 1 starts. */
        for (int k = 0; k < BENCH_PHASE_COUNT; ++k) {
            phase_stats_t *stats = &bench->phases[k];
            if (stats->boundary_sampled || now_us - t0_us < (int64_t)windows[k].start_ms * 1000) {
                continue;
            }
            gsp_counters(bench, &stats->gsp_frames_start, &stats->gsp_render_start,
                         &stats->gsp_submit_start);
            stats->boundary_sampled = true;
            phase_stats_t *prev = &bench->phases[k - 1];
            prev->gsp_frames = stats->gsp_frames_start - prev->gsp_frames_start;
            prev->gsp_render_us = stats->gsp_render_start - prev->gsp_render_start;
            prev->gsp_submit_us = stats->gsp_submit_start - prev->gsp_submit_start;
        }
        bench_frame_t frame;
        while (bench_probe_pop(&frame)) {
            account(bench, &frame, t0_us);
        }
        while (next_to_close < BENCH_PHASE_COUNT &&
                now_us - t0_us >= (int64_t)windows[next_to_close].end_ms * 1000 + PHASE_GRACE_US) {
            phase_stats_t *stats = &bench->phases[next_to_close];
            if (next_to_close == BENCH_PHASE_COUNT - 1) {
                uint32_t frames;
                uint64_t render_us, submit_us;
                gsp_counters(bench, &frames, &render_us, &submit_us);
                stats->gsp_frames = frames - stats->gsp_frames_start;
                stats->gsp_render_us = render_us - stats->gsp_render_start;
                stats->gsp_submit_us = submit_us - stats->gsp_submit_start;
            }
            stats->closed = true;
            print_phase(run, (bench_phase_t)next_to_close, stats);
            next_to_close++;
        }
    }
    print_memory(run);
}

static void bench_task(void *arg)
{
    bench_t *bench = arg;
    for (int run = 1; run <= BENCH_RUNS; ++run) {
        run_once(bench, run);
    }
    bench_clock_stop();
    /* The UI keeps idling; the bench state stays allocated until reboot. */
    vTaskDelete(NULL);
}

esp_err_t app_bench_start(esp_gsp_handle_t ui, esp_display_presenter_t *presenter,
                          const app_bench_hooks_t *hooks)
{
    ESP_RETURN_ON_FALSE(ui && presenter, ESP_ERR_INVALID_ARG, TAG, "invalid argument");
    ESP_RETURN_ON_FALSE(s_bench == NULL, ESP_ERR_INVALID_STATE, TAG, "already started");
    bench_t *bench = heap_caps_calloc(1, sizeof(*bench), MALLOC_CAP_INTERNAL);
    ESP_RETURN_ON_FALSE(bench != NULL, ESP_ERR_NO_MEM, TAG, "bench state");
    bench->phases = heap_caps_calloc(BENCH_PHASE_COUNT, sizeof(phase_stats_t), MALLOC_CAP_SPIRAM);
    if (bench->phases == NULL) {
        free(bench);
        return ESP_ERR_NO_MEM;
    }
    bench->ui = ui;
    bench->presenter = presenter;
    if (hooks) {
        bench->hooks = *hooks;
    }
    bench_probe_attach(presenter);
    /* Only BENCH lines from here on: the Iris log ring holds 4 KB. */
    esp_log_level_set("*", ESP_LOG_WARN);
    if (xTaskCreate(bench_task, "bench", TASK_STACK_BYTES, bench, TASK_PRIORITY, NULL) != pdPASS) {
        bench_probe_attach(NULL);
        free(bench->phases);
        free(bench);
        return ESP_ERR_NO_MEM;
    }
    s_bench = bench;
    return ESP_OK;
}
