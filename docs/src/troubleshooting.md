# Troubleshooting

| Problem | Fix |
| --- | --- |
| `Permission denied` on `/dev/ttyACM0` | Add yourself to `dialout`, log in again |
| `espflash` hangs on "Connecting" | Another program holds the serial port (a monitor): close it |
| Board not detected | Hold BOOT, tap RESET, release BOOT, flash again |
| Build fails: "No OTA key" or "no signing key" | `cargo iotkit keygen` |
| `ota push`: name not resolved | Use the device's address (`net` on the console) |
| `ota push`: connection timed out | The device is not on Wi-Fi yet, or on another network |
| Update refused | Another signing key, product or chip; a version not newer, or the running one older than `--min-version`; an image too big for the slot; "on trial": the last update is not healthy yet, wait for it to be kept or rolled back |
| No crash report, no early rollback, slow deep-sleep wake-ups, with code on the C6's LP core | The LP core overwrites iotkit's RTC memory: [not supported yet](battery.md#the-c6s-low-power-core-not-with-iotkit-yet) |
| Device keeps rolling back | The health check never passes: read the log over USB |
| Update over Matter fails: `CASE to … failed: TxTimeout`, `No valid session found` in the log | The provider's computer has two IPv6 addresses (global and ULA) and answers from the other one; rs-matter only takes the address it called. Route the Thread prefix from that address: `sudo ip -6 route add <prefix>/64 via <border router> dev <interface> src <address> metric 100` (`ip -6 route` lists the prefix and router; the address is in the CASE line) |
| Setup page cannot join | Wrong password or a 5 GHz-only network; some phone hotspots refuse for a minute after a wrong password |
| MQTT keeps reconnecting | Broker address (the log at boot, or `mqtt` on the console), login, or a weak Wi-Fi signal |
| A new `secrets.toml` value has no effect after a flash | A value given on the device wins over it: `cargo iotkit flash --factory-reset` ([where settings come from](getting-started.md#where-settings-come-from)) |
| `help` does nothing on the serial port | The console is opt-in: add the `console` feature |
| Browser install finds no port | Chrome or Edge on a computer, over HTTPS or localhost; a data USB cable |
| Browser install: *Failed to initialize* | Hold BOOT, tap RESET, release BOOT, then *Connect*. On Linux, keep ModemManager off the board: a udev rule `ATTRS{idVendor}=="303a", ENV{ID_MM_DEVICE_IGNORE}="1"` |
| Browser install done, no Wi-Fi step | Tap RESET (the board stayed in download mode), then *Connect* again |
