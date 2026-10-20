//! Polled sensors.

use crate::model::Sensor;
use embassy_time::{Duration, Ticker};

use crate::components::Filter;

/// Reads every `every`, passes the reading through `filter` and reports what comes out. `read`
/// returns `None` on a failed reading (the last value stays).
///
/// ```ignore
/// use components::filter::{Clamp, Linear};
/// // An ADC soil probe: 3000 dry, 1200 wet, as 0-100 %.
/// let percent = Linear::new((3000.0, 0.0), (1200.0, 100.0)).then(Clamp::new(0.0, 100.0));
/// components::poll(&SOIL, Duration::from_secs(30), percent, async || {
///     Some(f32::from(adc.read_oneshot(&mut pin).await))
/// })
/// .await
/// ```
pub async fn poll(
    entity: &Sensor,
    every: Duration,
    mut filter: impl Filter,
    mut read: impl AsyncFnMut() -> Option<f32>,
) -> ! {
    let mut ticker = Ticker::every(every);
    loop {
        if let Some(value) = read().await.and_then(|v| filter.apply(v)) {
            entity.set(value);
        }
        ticker.next().await;
    }
}
