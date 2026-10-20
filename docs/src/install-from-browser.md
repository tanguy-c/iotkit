# Install from the browser

Every `cargo iotkit build` also writes `dist/<package>/web/`: a merged image (bootloader,
partition table and firmware, flashed at offset 0), its [ESP Web Tools](https://esphome.github.io/esp-web-tools/)
manifest and an install page. Anyone with Chrome or Edge on a computer can then flash a device
over USB, with no Rust or espflash, and give it a Wi-Fi network right after (Improv over USB).

## Try it

```bash
cargo iotkit web --release      # build, then serve the page on http://localhost:8000/<package>/web/
```

Open the page, plug the device in, press *Connect*. The new install erases the flash (settings and
Matter pairings included), so it suits new devices; existing ones update [over the
network](updates.md).

Boards on the chip's own USB port (the ESP32-C6, C3, C5, S3 and H2 DevKits' USB connector):

- If the page cannot start (*Failed to initialize*), put the board in download mode first: hold
  BOOT, tap RESET, release BOOT, then *Connect*. On Linux, ModemManager opening the port is the
  usual cause ([troubleshooting](troubleshooting.md)).
- After *Installation complete*, tap RESET: the page's own reset leaves these boards in download
  mode. Then *Connect* again: the page finds the firmware over Improv and offers its Wi-Fi.

## Publish it

Copy `dist/<package>/web/` to any HTTPS site (Web Serial needs a secure page): GitHub Pages, a
NAS, your product's site. The page loads ESP Web Tools from unpkg; to host it yourself, change the
`<script>` line of `index.html`.

- The Wi-Fi step needs the `improv-serial` feature (on by default). Without it, the device opens
  its other setup paths ([getting started](getting-started.md#join-wi-fi)).
- One page per firmware and chip: `manifest.json` names the chip family, and ESP Web Tools refuses
  a board with another chip.
