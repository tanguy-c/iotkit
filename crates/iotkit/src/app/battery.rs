//! A battery's voltage with the ADC, and its charge (see [`BatteryCurve`]).

use crate::model::Sensor;
pub use crate::runtime::battery::{BatteryCurve, divided};
#[cfg(not(feature = "esp32"))]
use esp_hal::analog::adc::AdcCalCurve;
use esp_hal::{
    analog::adc::{Adc, AdcChannel, AdcConfig, AdcPin, Attenuation},
    gpio::AnalogPin,
    peripherals::ADC1,
};

/// Readings in millivolts (curve calibration); raw on the ESP32, which has none.
#[cfg(not(feature = "esp32"))]
type Calibration<'d> = AdcCalCurve<ADC1<'d>>;
#[cfg(feature = "esp32")]
type Calibration<'d> = ();

/// A battery on an ADC1 pin, behind a voltage divider. Read it before Wi-Fi starts: the radio's
/// load lowers the voltage.
pub struct Battery<'d, P> {
    adc: Adc<'d, ADC1<'d>, esp_hal::Blocking>,
    pin: AdcPin<P, ADC1<'d>, Calibration<'d>>,
    ratio: f32,
    curve: BatteryCurve,
}

impl<'d, P: AdcChannel + AnalogPin + 'd> Battery<'d, P> {
    const SAMPLES: u32 = 16;

    /// `divider_ratio`: battery voltage over pin voltage (2.0 for two equal resistors). A LiPo
    /// cell unless [`curve`](Self::curve) says otherwise.
    pub fn new(adc: ADC1<'d>, pin: P, divider_ratio: f32) -> Self {
        let mut config = AdcConfig::new();
        let pin = config.enable_pin_with_cal::<_, Calibration<'d>>(pin, Attenuation::_11dB);
        Self { adc: Adc::new(adc, config), pin, ratio: divider_ratio, curve: BatteryCurve::LIPO }
    }

    /// The battery chemistry: voltage to charge level.
    pub fn curve(mut self, curve: BatteryCurve) -> Self {
        self.curve = curve;
        self
    }

    /// The battery's voltage (the mean of 16 readings).
    pub fn voltage(&mut self) -> f32 {
        let mut sum = 0u32;
        for _ in 0..Self::SAMPLES {
            let millivolts = loop {
                if let Ok(mv) = self.adc.read_oneshot(&mut self.pin) {
                    // Uncalibrated 12-bit reading: about 3.1 V full scale at 11 dB, give or take
                    // 5 %, so the charge is a rough guide on the ESP32.
                    #[cfg(feature = "esp32")]
                    let mv = (u32::from(mv) * 3100 / 4095) as u16;
                    break mv;
                }
            };
            sum += u32::from(millivolts);
        }
        divided((sum / Self::SAMPLES) as u16, self.ratio)
    }

    /// Sets `percent` (whole percents) and `voltage`, if given; returns the voltage.
    pub fn report(&mut self, percent: &Sensor, voltage: Option<&Sensor>) -> f32 {
        let volts = self.voltage();
        let charge = self.curve.percent(volts);
        percent.set(if charge.is_nan() { charge } else { f32::from((charge + 0.5) as u8) });
        if let Some(sensor) = voltage {
            sensor.set(volts);
        }
        volts
    }
}
