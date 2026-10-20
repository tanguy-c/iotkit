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

## Publish it

Copy `dist/<package>/web/` to any HTTPS site (Web Serial needs a secure page): GitHub Pages, a
NAS, your product's site. The page loads ESP Web Tools from unpkg; to host it yourself, change the
`<script>` line of `index.html`.

- The Wi-Fi step needs the `improv-serial` feature (on by default). Without it, the device opens
  its other setup paths ([getting started](getting-started.md#join-wi-fi)).
- One page per firmware and chip: `manifest.json` names the chip family, and ESP Web Tools refuses
  a board with another chip.
