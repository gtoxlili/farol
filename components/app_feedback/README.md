# app_feedback

Carries out Farol's board feedback and reads its sensors: vibration patterns,
the three chimes, the interaction module (hot-plug, PIR, ambient light, touch
keys, six LEDs), the AI key, BMI270 motion, the BQ27220 fuel gauge, and panel
power and brightness. It decides nothing: when to vibrate, ring, glow or switch
the panel off is the logic's decision (`rust/logic`); the Rust glue is
`rust/platform/src/feedback.rs`.

```c
static void on_event(const app_feedback_event_t *event, void *ctx); /* on `feedback` */
ESP_ERROR_CHECK(app_feedback_start(on_event, ctx));
app_feedback_haptic(APP_FEEDBACK_HAPTIC_CALL);
app_feedback_chime(APP_FEEDBACK_CHIME_DONE);
app_feedback_glow(APP_FEEDBACK_GLOW_WORK);   /* only shown with the module */
app_feedback_led_brightness(60);
app_feedback_display(true, 80);
```

## Tasks and ownership

| Task | Priority, stack | Owns |
|---|---|---|
| `feedback` | 6, 5 KB | motor, AI key state, IMU, fuel gauge, module driver and LEDs, panel power (through `app_display`), the event callback |
| `chime` | 3, 3.5 KB | the ES8311 speaker path |

Requests (`app_feedback_*`) only write a mailbox under a spinlock and notify
`feedback`; they never block. The latest request wins, except that a FLASH
glow plays once over the current glow and a chime waits for the one playing
(one waiting slot, the newest replaces it). The button component's timer and
the module manager's tasks are BSP-owned; the AI key callbacks only notify
`feedback`. All hardware is brought up on `feedback` before `chime` brings up
the speaker, because BSP bring-up is not thread-safe. Each part is optional:
a part that fails to start is logged and stays off. Everything lives until
reboot. `app_feedback_start` returns `ESP_ERR_NO_MEM` with both tasks deleted
when one cannot be created.

## Behaviour

| Part | How | BSP |
|---|---|---|
| Haptics | step patterns timed by `feedback` from the planned step end: tick 10 ms, confirm 25 ms, call two 45 ms pulses at 55 %, fail 170 ms then 50 ms | `bsp_motor_*` (GPIO8, LEDC PWM) |
| Chimes | `tools/chimes.py` synthesizes 16 kHz mono PCM at build time (stdlib only, deterministic); the codec opens per chime and closes after 3 s idle, so the amplifier is off between chimes | `bsp_audio_init` (speaker only, mic DIN unassigned), `bsp_audio_codec_speaker_init` |
| AI key | press, 600 ms long press, release; a click is a release without a long press; a key held through boot is ignored until released | `bsp_iot_button_create` (GPIO7) |
| Motion | accelerometer polled every 50 ms; `LIFTED` when a board that sat still for 1.5 s tilts 12° or its acceleration departs 0.15 g from 1 g; `FACE_DOWN` when the screen faces down (z < -0.85 g share) for 0.8 s, `FACE_UP` when it leaves that (z > -0.5) for 0.3 s | `bsp_imu_*` (BMI270, 50 Hz, 2 g) |
| Power | every 5 s; reported on a change of percent, charging or availability, a change of 2+ minutes in the estimate, and at least every 30 s; charging follows the factory firmware (DSG flag, positive current, 3 confirming samples); `known=false` while the gauge does not answer (retried every 30 s) | `bsp_battery_init`, `bsp_battery_read` (BQ27220) |
| Module | see below | `mosaico_module_mgr`, `mosaico_module_interact` |
| Display | on: brightness, then panel on; off: panel off; repeats are skipped | `app_display_set_brightness`, `app_display_set_enabled` |

### Interaction module

Optional and hot-pluggable, right slot only. The module manager probes both
slots' EEPROMs every 250 ms on its own task and changes state after three
matching probes. `feedback` reads the right slot's snapshot every 250 ms: an
Interaction descriptor opens the driver, which claims the slot (failed opens
retry every 5 s), and `ABSENT` closes it; the manager keeps probing a claimed
Interaction board, so removal is seen while it is open (about 0.75 s).
`MODULE` is reported once the slot has settled, at boot and on every change.
The left slot (camera) is never claimed here.

While attached, inputs are read every 50 ms: touch keys report `PREV` (left
key) and `NEXT` (right key) on press; the PIR reports `PRESENCE` on a change
seen twice; the driver's filtered light level is reported as `AMBIENT` on
steps of 4 or more, at most once a second. The LEDs are written at about
30 fps while a glow breathes or flashes, and only when the frame changes:

| Glow | Look |
|---|---|
| WORK | moonlight blue, very dim, 4 s breathing |
| CALL | amber, 2.4 s breathing |
| FAIL | ember red, steady |
| FLASH | white, 60 ms rise, 140 ms hold, 400 ms fade, back to the glow below |

The driver's brightness is fixed at 96/255; the LED percentage scales below it.

## Logs

Tag `feedback`: `ready`, each haptic and chime, AI key and touch keys,
`presence`, `motion`, `power` changes, module attach and removal, glow changes,
display on and off, and every 60 s a `status:` line with the battery, the IMU's
z share and the module's state, ambient level, glow and LED brightness.
Ambient steps and brightness-only display changes are debug-level.

## Memory

Two task stacks (8.5 KB) in internal RAM, plus the BSP module manager's two
tasks (8 KB) and the driver's small state. The chimes are 35 KB of flash.
