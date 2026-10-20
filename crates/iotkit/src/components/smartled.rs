//! Addressable LEDs (WS2812 and others, through `smart-leds`).

use crate::model::{Entity, Light, LightState};
use log::warn;
use smart_leds::{RGB8, SmartLedsWriteAsync, brightness};

/// `count` LEDs showing one colour, as an RGB light. Starts off, white at full brightness.
///
/// With esp-hal: an `esp_hal_smartled::RmtSmartLeds` on an RMT channel.
pub async fn rgb_light<L>(entity: &Light, mut leds: L, count: usize) -> !
where
    L: SmartLedsWriteAsync<Color = RGB8>,
    L::Error: core::fmt::Debug,
{
    let mut state = LightState { on: false, brightness: 255, rgb: [255, 255, 255] };
    loop {
        let color = if state.on { RGB8::new(state.rgb[0], state.rgb[1], state.rgb[2]) } else { RGB8::default() };
        if let Err(e) = leds.write(brightness(core::iter::repeat_n(color, count), state.brightness)).await {
            warn!("{}: rgb_light write failed: {e:?}", entity.meta().id);
        }
        entity.set(state);
        state = entity.wait_command().await;
    }
}
