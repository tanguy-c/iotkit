# Troubleshooting

| Problem | Fix |
| --- | --- |
| `Permission denied` on `/dev/ttyACM0` | Add yourself to `dialout`, log in again |
| `espflash` hangs on "Connecting" | Another program holds the serial port (a monitor): close it |
| Board not detected | Hold BOOT, tap RESET, release BOOT, flash again |
| Build fails: "No OTA key" or "no signing key" | `cargo iotkit keygen` |
| `ota push`: name not resolved | Use the device's address (`net` on the console) |
| `ota push`: connection timed out | The device is not on Wi-Fi yet, or on another network |
| Update refused | Another signing key, product or chip; a version not newer, or the running one older than `--min-version`; an image too big for the slot |
| Device keeps rolling back | The health check never passes: read the log over USB |
| Setup page cannot join | Wrong password or a 5 GHz-only network; some phone hotspots refuse for a minute after a wrong password |
| MQTT keeps reconnecting | Broker address (`mqtt` on the console), login, or a weak Wi-Fi signal |
| Browser install finds no port | Chrome or Edge on a computer, over HTTPS or localhost; a data USB cable |
| Browser install: *Failed to initialize* | Hold BOOT, tap RESET, release BOOT, then *Connect*. On Linux, keep ModemManager off the board: a udev rule `ATTRS{idVendor}=="303a", ENV{ID_MM_DEVICE_IGNORE}="1"` |
| Browser install done, no Wi-Fi step | Tap RESET (the board stayed in download mode), then *Connect* again |
