# app_bench

Frame benchmark harness for Farol: scripted input, frame measurements and `BENCH`
lines on stdout. Built only with `CONFIG_FAROL_BENCH` (menu "Farol"), which also
selects `CONFIG_FAROL_DEMO`, so the board runs the demo fleet with nobody at it. With
the option off the component is empty: no sources, no linker wraps, no fake touch.

- `bench_script.c`: the run timeline and a fake `esp_lcd_touch` device. GSP polls it
  (`ESP_GSP_TOUCH_INPUT_POLLING`) instead of the panel's touch; each read samples the
  script on a 10 ms grid of `esp_timer` time. Each drag's release is the sample after
  it reaches its end point.
- `bench_probe.c`: linker wraps (see `CMakeLists.txt`) around
  `esp_display_presenter_begin_next_frame` (render start),
  `esp_display_presenter_commit_frame_with_receipt` (render end, submit start; frame id
  from the receipt; ESP-GSP 1.9.0 commits only through this call, not `commit_frame`)
  and the presenter's `esp_display_present_tracker_complete_present_frame_isr` (TE
  compose pool retires the frame when its transfer is done). The last one is an internal
  symbol of esp_display_present 1.2.0; re-check the three when ESP-GSP or that component
  changes: a wrap nobody calls links silently and the phase lines report zero frames.
- `app_bench.c`: the `bench` task (priority 2) runs three runs back to back, assigns
  frames to the phase in which their presentation completed, and prints one line per
  phase 200 ms after it ends, then a memory line per run.

## Timeline

One run, ms from its start; screen pixels of the 480x480 panel. Drags take 300 ms.

| Phase | Start | End | Input |
|---|---:|---:|---|
| `warmup` | 0 | 5000 | none (demo data arrives, home page) |
| `idle_home` | 5000 | 10000 | none (lamp at rest) |
| `to_deck` | 10000 | 10910 | drag (400,300)→(80,300), 600 ms settle |
| `deck_idle` | 10910 | 13910 | none |
| `deck_flick` | 13910 | 16640 | 3× drag (240,380)→(240,100), 600 ms settle each |
| `running_idle` | 16640 | 19640 | none (running card: shimmer, timer) |
| `hold_stop` | 19640 | 21940 | hold (240,420) 1700 ms, 600 ms after release |
| `to_home` | 21940 | 22850 | drag (80,300)→(400,300), 600 ms settle |
| `control` | 22850 | 24670 | drawer drag (240,6)→(240,380), then (240,370)→(240,20), 600 ms settle each |
| `to_usage` | 24670 | 28490 | drag (80,300)→(400,300), settle, 2 s idle, drag back, settle |
| `idle_end` | 28490 | 33490 | none |

Every run starts from the same scene: `main` passes a `reset_scene` hook that restarts
the demo fleet 14 s into its script on the home page (`app_logic_restart_demo`). At
that moment one agent asks a question and one is mid-turn, so the third flick lands on
the running card and the hold stops it.

## Output

One JSON object per line after `BENCH `. Per phase: `frames`; `fps` = presented frames
per second from the transfer-done intervals; `interval_us` (between consecutive
transfer-done times), `render_us` (begin_next_frame returned -> commit entered)
and `present_us` (commit entered -> transfer done) as p50/p95/max. `latency_us` is per
touch sample (presses, moves and the release sample): from the sample's grid time to
the transfer-done time of the first frame whose drawing began after GSP read that
sample; it is zero for phases without input. `gsp` carries GSP's own cumulative
counters over the phase (`esp_gsp_render_stats`, `esp_gsp_render_phases`). Per run,
`mem` gives internal RAM and PSRAM free and minimum free bytes.

Targets: latency p50 ≤ 45 ms and p95 ≤ 80 ms; lamp phases ≥ 25 fps; internal RAM
minimum free ≥ 60 KB.

Calls: `app_bench_touch_create` once before GSP starts; `app_bench_start` once after the
logic started (hooks may be NULL). `app_bench_start` sets the log level to WARN so only
BENCH lines reach the 4 KB Iris log ring. State lives until reboot.
