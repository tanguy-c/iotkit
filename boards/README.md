# Boards

What `cargo iotkit new` knows about each board: one TOML file per board, its name the board's id
(`board = "xiao-esp32c6"` in an answers file). The CLI embeds this folder when it is built.

The chip's own rules (which pins hold the flash or the USB port, which are strapping pins or have
an ADC) are not here: they are the same on every board with that chip, in
`xtask/src/new/boards.rs`. A board file says how the board is wired around its chip.

```toml
label    = "Maker Board-name (chip, flash; what stands out)"  # the menu, searched as you type
chip     = "esp32c6"           # esp32c6, esp32c3, esp32c5, esp32s3, esp32h2 or esp32
model    = "Board-name"        # Home Assistant's model
flash    = "4mb"               # 4mb or 8mb (8 MB or more); left out: asked
boot_pin = 9                   # the BOOT button: the setup button
order    = 60                  # place in the menu (lowest first)
led      = { kind = "rgb", pin = 8 }                     # a WS2812
# led    = { kind = "plain", pin = 15, active = "low" }  # a single LED, lit by a low level
sources  = ["https://..."]      # where each fact comes from: two at least

[[init]]                       # a pin set in `main` before the radio starts
pin   = 3
level = "low"
why   = "powers the antenna switch"
```

Check each fact against two sources: the maker's documentation or schematic, Zephyr's board
devicetree (`boards/<vendor>/<board>/*.dts`: LEDs with their polarity, buttons, flash size,
switches), and Arduino's board variant (`variants/<board>/pins_arduino.h`, `variant.cpp`).
`cargo test --manifest-path xtask/Cargo.toml --target host-tuple` checks every file against its
chip's rules.

## Waiting for a second source

Boards asked for whose wiring only one source gives in text so far:

| Board | One source says | Missing |
| --- | --- | --- |
| Waveshare ESP32-C6-Zero | RGB LED on GPIO8 (Arduino's variant) | Waveshare's pinout is an image |
| LOLIN C3 Mini | RGB LED on GPIO7 (Arduino's variant) | which board revision has which LED |
| Unexpected Maker TinyC6 | RGB LED on GPIO23, powered by GPIO22 (Arduino's variant) | Unexpected Maker's pinout card |
| Unexpected Maker TinyS3 | RGB LED on GPIO18, powered by GPIO17 (Arduino's variant) | the same; the power pin's level |
| Arduino Nano ESP32 | LED on GPIO48; RGB LED on GPIO46, 0, 45 (Arduino's variant) | the LEDs' polarity; a three-pin RGB LED is not a kind `boards/` knows yet |
