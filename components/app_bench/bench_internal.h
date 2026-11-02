// SPDX-License-Identifier: Apache-2.0
#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "esp_display_present.h"

/* Timeline of one run, in milliseconds from the run start (bench_script.c). */
typedef enum {
    BENCH_PHASE_WARMUP,
    BENCH_PHASE_IDLE_HOME,
    BENCH_PHASE_TO_DECK,
    BENCH_PHASE_DECK_IDLE,
    BENCH_PHASE_DECK_FLICK,
    BENCH_PHASE_RUNNING_IDLE,
    BENCH_PHASE_HOLD_STOP,
    BENCH_PHASE_TO_HOME,
    BENCH_PHASE_CONTROL,
    BENCH_PHASE_TO_USAGE,
    BENCH_PHASE_IDLE_END,
    BENCH_PHASE_COUNT,
} bench_phase_t;

typedef struct {
    const char *name;
    uint32_t start_ms;
    uint32_t end_ms;
} bench_phase_window_t;

const bench_phase_window_t *bench_script_phases(void);
uint32_t bench_script_run_ms(void);

/* Run clock shared by the bench task (writer) and the render task (reader). */
void bench_clock_start_run(int64_t t0_us);
void bench_clock_stop(void);

/* Touch samples delivered to GSP but not yet claimed by a frame. Render task. */
#define BENCH_FRAME_SAMPLES_MAX 16 /* 160 ms of 10 ms samples per frame */

/* One presented frame as seen by the probes (bench_probe.c). */
typedef struct {
    uint64_t id;
    int64_t begin_us;    /* presenter begin_next_frame returned: drawing starts */
    int64_t commit_us;   /* commit entered: drawing done, submit starts */
    int64_t done_us;     /* present completion (transfer done), 0 until known */
    uint8_t sample_count;
    int64_t samples_us[BENCH_FRAME_SAMPLES_MAX];
} bench_frame_t;

/* Probe hooks for bench_script.c's touch device (render task). */
void bench_probe_touch_sample(int64_t sample_us);

/* Bench task side: attach the presenter, then pop completed frames in order. */
void bench_probe_attach(esp_display_presenter_t *presenter);
bool bench_probe_pop(bench_frame_t *out_frame);
