//! I²C sensor chips. Each call starts a measurement, waits for it and returns the reading: `None`
//! on a bus error or a bad checksum. They keep no state, so a battery device reads them once per
//! wake-up; generic over embedded-hal-async's `I2c` (esp-hal's `I2c<Async>`, or an `I2cDevice`
//! on a shared bus).
//!
//! ```ignore
//! // Plugged in: every 30 s, on the bus the parts share.
//! components::poll(&LIGHT, Duration::from_secs(30), |v| Some(v), async || {
//!     components::i2c::bh1750(&mut dev, 0x23).await
//! })
//! .await
//! ```

use embassy_time::Timer;
use embedded_hal_async::i2c::I2c;

/// A temperature and humidity reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempHumidity {
    /// °C.
    pub temperature: f32,
    /// Relative humidity, %.
    pub humidity: f32,
}

/// A BME280's reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bme280 {
    /// °C.
    pub temperature: f32,
    /// Relative humidity, %.
    pub humidity: f32,
    /// hPa.
    pub pressure: f32,
}

/// A BMP280's reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bmp280 {
    /// °C.
    pub temperature: f32,
    /// hPa.
    pub pressure: f32,
}

/// An SCD4x's reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scd4x {
    /// ppm.
    pub co2: f32,
    /// °C.
    pub temperature: f32,
    /// Relative humidity, %.
    pub humidity: f32,
}

/// A ROHM BH1750's light level, in lux (address 0x23, or 0x5C with ADDR high): one
/// high-resolution measurement (1 lx, 180 ms), after which it powers down.
pub async fn bh1750(i2c: &mut impl I2c, address: u8) -> Option<f32> {
    // Power on, then one measurement.
    i2c.write(address, &[0x01]).await.ok()?;
    i2c.write(address, &[0x20]).await.ok()?;
    Timer::after_millis(180).await;
    let mut raw = [0; 2];
    i2c.read(address, &mut raw).await.ok()?;
    Some(f32::from(u16::from_be_bytes(raw)) / 1.2)
}

/// A Sensirion SHT40/41/45 (address 0x44, 0x45 or 0x46 by its part number): one
/// high-precision measurement (8.3 ms).
pub async fn sht4x(i2c: &mut impl I2c, address: u8) -> Option<TempHumidity> {
    let [t, rh] = sensirion_words(i2c, address, &[0xFD], 10).await?;
    Some(TempHumidity { temperature: celsius(t), humidity: (-6.0 + 125.0 * ratio(rh)).clamp(0.0, 100.0) })
}

/// A Sensirion SHT30/31/35 (address 0x44, or 0x45 with ADDR high): one high-repeatability
/// measurement without clock stretching (15.5 ms).
pub async fn sht3x(i2c: &mut impl I2c, address: u8) -> Option<TempHumidity> {
    let [t, rh] = sensirion_words(i2c, address, &[0x24, 0x00], 16).await?;
    Some(TempHumidity { temperature: celsius(t), humidity: 100.0 * ratio(rh) })
}

/// An Aosong AHT20/21/25 (address 0x38): calibrated if it is not, then one measurement
/// (80 ms).
pub async fn aht20(i2c: &mut impl I2c, address: u8) -> Option<TempHumidity> {
    let mut status = [0];
    i2c.read(address, &mut status).await.ok()?;
    if status[0] & 0x08 == 0 {
        i2c.write(address, &[0xBE, 0x08, 0x00]).await.ok()?;
        Timer::after_millis(10).await;
    }
    i2c.write(address, &[0xAC, 0x33, 0x00]).await.ok()?;
    Timer::after_millis(80).await;
    let mut b = [0; 7];
    for _ in 0..5 {
        i2c.read(address, &mut b).await.ok()?;
        if b[0] & 0x80 == 0 {
            break;
        }
        Timer::after_millis(10).await;
    }
    if b[0] & 0x80 != 0 || crc8(&b[..6]) != b[6] {
        return None;
    }
    let rh = u32::from(b[1]) << 12 | u32::from(b[2]) << 4 | u32::from(b[3]) >> 4;
    let t = u32::from(b[3] & 0x0F) << 16 | u32::from(b[4]) << 8 | u32::from(b[5]);
    let scale = (1u32 << 20) as f32;
    Some(TempHumidity { temperature: t as f32 / scale * 200.0 - 50.0, humidity: rh as f32 / scale * 100.0 })
}

/// A Bosch BME280 (address 0x76, or 0x77 with SDO high): one forced measurement, 1×
/// oversampling (10 ms). `None` for a BMP280, which has no humidity ([`bmp280`]).
pub async fn bme280(i2c: &mut impl I2c, address: u8) -> Option<Bme280> {
    let (temperature, pressure, humidity) = bmx280(i2c, address, true).await?;
    Some(Bme280 { temperature, humidity: humidity?, pressure })
}

/// A Bosch BMP280 (address 0x76, or 0x77 with SDO high), or a BME280 without its humidity: one
/// forced measurement, 1× oversampling.
pub async fn bmp280(i2c: &mut impl I2c, address: u8) -> Option<Bmp280> {
    let (temperature, pressure, _) = bmx280(i2c, address, false).await?;
    Some(Bmp280 { temperature, pressure })
}

/// A Sensirion SCD40/41 (address 0x62), in low-power periodic mode (a measurement every 30 s,
/// started on the first call and kept running): waits for the next one, 35 s at most.
pub async fn scd4x(i2c: &mut impl I2c, address: u8) -> Option<Scd4x> {
    // Refused (an error) while it already runs.
    let _ = i2c.write(address, &0x21ACu16.to_be_bytes()).await;
    let mut ready = false;
    for _ in 0..36 {
        let [status] = sensirion_words(i2c, address, &0xE4B8u16.to_be_bytes(), 1).await?;
        ready = status & 0x07FF != 0;
        if ready {
            break;
        }
        Timer::after_secs(1).await;
    }
    if !ready {
        return None;
    }
    let [co2, t, rh] = sensirion_words(i2c, address, &0xEC05u16.to_be_bytes(), 1).await?;
    Some(Scd4x { co2: f32::from(co2), temperature: celsius(t), humidity: 100.0 * ratio(rh) })
}

/// Sends `command`, waits `wait_ms`, then reads `N` words, each followed by its CRC.
async fn sensirion_words<const N: usize>(
    i2c: &mut impl I2c,
    address: u8,
    command: &[u8],
    wait_ms: u64,
) -> Option<[u16; N]> {
    i2c.write(address, command).await.ok()?;
    Timer::after_millis(wait_ms).await;
    let mut raw = [0u8; 9];
    let raw = &mut raw[..3 * N];
    i2c.read(address, raw).await.ok()?;
    let mut words = [0; N];
    for (word, chunk) in words.iter_mut().zip(raw.chunks(3)) {
        if crc8(&chunk[..2]) != chunk[2] {
            return None;
        }
        *word = u16::from_be_bytes([chunk[0], chunk[1]]);
    }
    Some(words)
}

/// Sensirion's (and Aosong's) CRC-8: polynomial 0x31, initial value 0xFF.
pub(super) fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0xFFu8;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { crc << 1 ^ 0x31 } else { crc << 1 };
        }
    }
    crc
}

fn ratio(raw: u16) -> f32 {
    f32::from(raw) / 65535.0
}

/// A Sensirion temperature word, in °C.
fn celsius(raw: u16) -> f32 {
    -45.0 + 175.0 * ratio(raw)
}

/// °C, hPa and, from a BME280 when `humidity`, %.
async fn bmx280(i2c: &mut impl I2c, address: u8, humidity: bool) -> Option<(f32, f32, Option<f32>)> {
    let mut id = [0];
    i2c.write_read(address, &[0xD0], &mut id).await.ok()?;
    let bme = id[0] == 0x60;
    if humidity && !bme {
        log::warn!("BME280 at {address:#04x}: chip id {:#04x}, not a BME280 (a BMP280 has no humidity)", id[0]);
        return None;
    }
    let mut c = [0; 26];
    i2c.write_read(address, &[0x88], &mut c).await.ok()?;
    let mut h = [0; 7];
    if bme {
        i2c.write_read(address, &[0xE1], &mut h).await.ok()?;
        // Humidity oversampling 1×: applies at the next write of ctrl_meas.
        i2c.write(address, &[0xF2, 0x01]).await.ok()?;
    }
    // Temperature and pressure oversampling 1×, forced mode: one measurement, then sleep.
    i2c.write(address, &[0xF4, 0x25]).await.ok()?;
    Timer::after_millis(10).await;
    let mut status = [0];
    for _ in 0..5 {
        i2c.write_read(address, &[0xF3], &mut status).await.ok()?;
        if status[0] & 0x08 == 0 {
            break;
        }
        Timer::after_millis(5).await;
    }
    let mut d = [0; 8];
    let d = if bme { &mut d[..] } else { &mut d[..6] };
    i2c.write_read(address, &[0xF7], d).await.ok()?;
    let calib = Calibration::new(&c, &h);
    let raw20 = |b: &[u8]| (i32::from(b[0]) << 12) | (i32::from(b[1]) << 4) | (i32::from(b[2]) >> 4);
    let (t_fine, centi) = calib.temperature(raw20(&d[3..6]));
    let pressure = calib.pressure(raw20(&d[0..3]), t_fine)? as f32 / 25600.0;
    let humidity = bme.then(|| calib.humidity((i32::from(d[6]) << 8) | i32::from(d[7]), t_fine) as f32 / 1024.0);
    Some((centi as f32 / 100.0, pressure, humidity))
}

/// The trimming parameters a BMx280 stores (`dig_T1` ...), and Bosch's integer compensation
/// (datasheet, section 4.2.3).
struct Calibration {
    t1: i32,
    t2: i32,
    t3: i32,
    p: [i64; 9],
    h1: i32,
    h2: i32,
    h3: i32,
    h4: i32,
    h5: i32,
    h6: i32,
}

impl Calibration {
    /// From the 26 bytes at 0x88 and the 7 at 0xE1 (zeros on a BMP280).
    fn new(c: &[u8; 26], h: &[u8; 7]) -> Self {
        let u = |i: usize| i64::from(u16::from_le_bytes([c[i], c[i + 1]]));
        let s = |i: usize| i64::from(i16::from_le_bytes([c[i], c[i + 1]]));
        let mut p = [0; 9];
        p[0] = u(6);
        for (k, v) in p.iter_mut().enumerate().skip(1) {
            *v = s(6 + 2 * k);
        }
        Self {
            t1: u(0) as i32,
            t2: s(2) as i32,
            t3: s(4) as i32,
            p,
            h1: i32::from(c[25]),
            h2: i32::from(i16::from_le_bytes([h[0], h[1]])),
            h3: i32::from(h[2]),
            h4: (i32::from(h[3] as i8) << 4) | i32::from(h[4] & 0x0F),
            h5: (i32::from(h[5] as i8) << 4) | i32::from(h[4] >> 4),
            h6: i32::from(h[6] as i8),
        }
    }

    /// `(t_fine, temperature in 0.01 °C)`.
    fn temperature(&self, adc: i32) -> (i32, i32) {
        let var1 = (((adc >> 3) - (self.t1 << 1)) * self.t2) >> 11;
        let var2 = (((((adc >> 4) - self.t1) * ((adc >> 4) - self.t1)) >> 12) * self.t3) >> 14;
        let t_fine = var1 + var2;
        (t_fine, (t_fine * 5 + 128) >> 8)
    }

    /// Pa in Q24.8.
    fn pressure(&self, adc: i32, t_fine: i32) -> Option<u32> {
        let p = &self.p;
        let mut var1 = i64::from(t_fine) - 128000;
        let mut var2 = var1 * var1 * p[5];
        var2 += (var1 * p[4]) << 17;
        var2 += p[3] << 35;
        var1 = ((var1 * var1 * p[2]) >> 8) + ((var1 * p[1]) << 12);
        var1 = (((1i64 << 47) + var1) * p[0]) >> 33;
        if var1 == 0 {
            return None;
        }
        let mut pa = 1048576 - i64::from(adc);
        pa = (((pa << 31) - var2) * 3125) / var1;
        let var1 = (p[8] * (pa >> 13) * (pa >> 13)) >> 25;
        let var2 = (p[7] * pa) >> 19;
        Some((((pa + var1 + var2) >> 8) + (p[6] << 4)) as u32)
    }

    /// % in Q22.10.
    fn humidity(&self, adc: i32, t_fine: i32) -> u32 {
        let x = t_fine - 76800;
        let a = (((adc << 14) - (self.h4 << 20) - (self.h5 * x)) + 16384) >> 15;
        let b = ((x * self.h6) >> 10) * (((x * self.h3) >> 11) + 32768);
        let b = (((b >> 10) + 2097152) * self.h2 + 8192) >> 14;
        let mut v = a * b;
        v -= ((((v >> 15) * (v >> 15)) >> 7) * self.h1) >> 4;
        (v.clamp(0, 419430400) >> 12) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc() {
        // Sensirion's datasheet example.
        assert_eq!(crc8(&[0xBE, 0xEF]), 0x92);
    }

    /// The BMP280 datasheet's example (section 3.12): 25.08 °C, 100653.27 Pa.
    #[test]
    fn bmp280_compensation() {
        let mut c = [0u8; 26];
        let words: [i32; 12] = [27504, 26435, -1000, 36477, -10685, 3024, 2855, 140, -7, 15500, -14600, 6000];
        for (i, w) in words.iter().enumerate() {
            c[2 * i..2 * i + 2].copy_from_slice(&(*w as u16).to_le_bytes());
        }
        let calib = Calibration::new(&c, &[0; 7]);
        let (t_fine, centi) = calib.temperature(519888);
        assert_eq!((t_fine, centi), (128422, 2508));
        let pa = calib.pressure(415148, t_fine).unwrap() as f32 / 256.0;
        assert!((pa - 100653.27).abs() < 0.5, "{pa}");
    }

    /// Humidity stays in 0-100 % (Q22.10) for a typical BME280's trimming.
    #[test]
    fn bme280_humidity() {
        let mut c = [0u8; 26];
        c[25] = 75;
        let h = [0x6A, 0x01, 0x00, 0x13, 0x2F, 0x03, 0x1E];
        let calib = Calibration::new(&c, &h);
        assert_eq!((calib.h2, calib.h4, calib.h5), (362, 0x13F, 0x032));
        for adc in [20000, 30000, 40000] {
            let rh = calib.humidity(adc, 128422) as f32 / 1024.0;
            assert!((0.0..=100.0).contains(&rh), "{adc}: {rh}");
        }
    }
}
