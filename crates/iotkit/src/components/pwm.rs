//! PWM parts (esp-hal: a LEDC channel): a dimmable light, a fan.

use crate::model::{Fan, FanState, Light, LightState};
use embedded_hal::pwm::SetDutyCycle;

/// A dimmable single-colour light. Starts off at full brightness; the duty follows brightness²
/// (closer to how the eye sees it than a linear duty).
pub async fn dimmer(entity: &Light, mut pwm: impl SetDutyCycle) -> ! {
    let mut state = LightState { on: false, brightness: 255, rgb: [255, 255, 255] };
    loop {
        let level = if state.on { u16::from(state.brightness) } else { 0 };
        let _ = pwm.set_duty_cycle_fraction(level * level, 255 * 255);
        entity.set(state);
        state = entity.wait_command().await;
    }
}

/// A fan driven by PWM: the duty is its speed (full speed for a fan without `speed()`). Starts
/// off.
pub async fn fan(entity: &Fan, mut pwm: impl SetDutyCycle) -> ! {
    let mut state = FanState::OFF;
    loop {
        let _ = match (state.on, state.percentage) {
            (false, _) => pwm.set_duty_cycle_fully_off(),
            (true, 0) => pwm.set_duty_cycle_fully_on(),
            (true, p) => pwm.set_duty_cycle_percent(p.min(100)),
        };
        entity.set(state);
        state = entity.wait_command().await;
    }
}
