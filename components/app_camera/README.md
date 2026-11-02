# app_camera

Scan sessions for the scan page: the camera module in the left slot, a live
preview inside the viewfinder, and QR decoding with quirc. API:
`include/app_camera.h`.

## Building with the camera

The camera driver, the BSP's `mosaico_module_camera`, patches two things while
CMake configures the project:

- `$IDF_PATH`: `patches/esp-idf-dvp-frame-capture-stability.patch` (DVP frame
  tail and FIFO reset in `esp_driver_cam`). Without it the build still passes,
  but the SC101IOT sensor drops or tears frames.
- `managed_components/espressif__esp_cam_sensor` in this project (SC101IOT
  PCLK delay). That copy belongs to the project, so nothing else is affected.

Patching ESP-IDF changes every project built from that checkout, so Farol's
root `CMakeLists.txt` stops the configure when `$IDF_PATH` lacks the patch,
unless `FAROL_ALLOW_IDF_PATCH=1` is set. Build from a private copy of the
pinned ESP-IDF and its tools, and let the first configure patch it:

```sh
D=/path/to/private-idf                # anywhere; same APFS volume = instant clone
mkdir -p "$D"
cp -c -R "$IDF_PATH" "$D/esp-idf"
cp -c -R "$IDF_TOOLS_PATH" "$D/tools"
# Register the copy in the copied tools' idf-env.json (next to the original entry):
python3 - "$D" <<'EOF'
import json, sys
d = sys.argv[1]; p = f"{d}/tools/idf-env.json"; env = json.load(open(p))
env["idfInstalled"][f"{d}/esp-idf-v6.2"] = {"version": "6.2", "path": f"{d}/esp-idf",
                                           "features": ["core"], "targets": ["esp32s31"]}
json.dump(env, open(p, "w"), indent=4)
EOF
export IDF_PATH=$D/esp-idf IDF_TOOLS_PATH=$D/tools
export IDF_PYTHON_ENV_PATH=$D/tools/python_env/idf6.2_py3.13_env
# First build only: allow the camera component to patch this copy.
FAROL_ALLOW_IDF_PATCH=1 python mosaico.py ...   # or idf_low_noise_build.py ... build
```

Later builds, `system-update` included, need only the three exports; the copy
then carries the patch (`git -C $IDF_PATH status` lists three files under
`components/esp_driver_cam/dvp`). The configure log reports
`mosaico_module_camera: applied ESP-IDF DVP patch` once and
`... patch already applied` afterwards. A copy made with `cp -c` on another
volume is a full copy (about 4 GB with the tools).

## Session

`app_camera_start()` records the request and creates the `app_camera` task
(core 1, priority 3, 6 KB internal stack) unless one runs; it returns at once.
The task:

1. claims the left slot with `mosaico_camera_new()`, which takes the slot's
   control GPIO (the camera's D4) from the module manager for the session and
   hands it back in `mosaico_camera_del()`; app_feedback's hot-plug scanning of
   the right slot keeps running;
2. streams the sensor's default format (`sdkconfig.defaults`: OV3640 640x480 at
   30 fps; SC101IOT 1280x720 at 25 fps), UYVY, two capture buffers;
3. per frame: the PPA crops the largest centred square that scales to 360 px in
   its 1/16 steps (480 from VGA, 720 from 720p), rotates it 90° as the factory
   camera app does, converts to RGB565 and the frame goes to the scene's Canvas
   `scan_preview` (`esp_gsp_canvas_try_submit_frame`); every 100 ms, when the
   decoder is idle, the same crop's luma (box-averaged to at most 480 px) goes to
   the decoder;
4. on `app_camera_stop()`: stops the decoder, `esp_gsp_canvas_stop()` restores
   the placeholder, waits for GSP's release callbacks, frees the buffers,
   releases the camera and the slot, and exits — or opens a new session if a
   start arrived meanwhile.

If the start fails (no module claimed in the left slot, sensor not detected,
stream not started, or no memory for the task), the error is logged, the
subscribed `on_failed` callback runs once (the platform posts
`Event::NoCamera`), and the task exits; the next `app_camera_start()` tries
again. A normal stop reports nothing, and neither does a capture failure after
the stream ran (it is logged and the session ends).

The decoder (`cam_qr.c`) runs quirc on the `app_cam_qr` task (core 1, priority
2, 16 KB stack in PSRAM), tries a mirrored read when error correction fails,
and reports each distinct payload once per session (FNV-1a hashes of the last
8) through the subscribed callback, on that task. Payloads are never logged.

## Memory per session (PSRAM unless noted)

| What | Bytes |
|---|---|
| Capture buffers, 2 x frame | 1.2 M (VGA) / 3.7 M (720p) |
| Preview frames, 2 x 360x360 RGB565 | 518 K |
| quirc image and labels | 230 K (VGA) / 130 K (720p) + flood-fill stack |
| quirc result structs, decoder stack | 13 K + 16 K |
| Session task stack (internal) | 6 K |

All of it is freed when the session ends. The UI bundle carries the 360x360 raw
placeholder (+276 KB).

## Logs (tag `app_camera`)

- `camera on in N ms: WxH @ F fps, crop C, preview 360, QR SxS`
- every 5 s while scanning: camera fps, preview fps (frames skipped because GSP
  still held both), QR searches per second and their average time, codes
  decoded and how many were new
- `QR decoded: Farol|Wi-Fi|other code, N bytes`
- `camera off after N ms: frames, preview fps, codes reported`

## Threads and ownership

The service is a singleton: `app_main` calls `app_camera_init()` once with the
UI, the Rust platform subscribes, the logic task starts and stops sessions. A
mutex guards the request; the session task owns the camera, the PPA client and
the buffers; GSP owns a preview frame from submission until its release
callback (render task), tracked by one atomic flag per frame.
