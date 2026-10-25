//! Local time from a POSIX time zone, as Home Assistant sends it over the ESPHome API
//! (`GetTimeResponse.parsed_timezone`): the offsets, and when daylight saving time starts and ends.
//! Times are Unix seconds (UTC).

/// The day a daylight saving time transition falls on, in a year.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Day {
    /// No transition: no daylight saving time.
    #[default]
    None,
    /// `Mm.w.d`: day `weekday` (0 Sunday) of week `week` (1 to 5, 5 the last) of `month` (1-12).
    MonthWeekDay {
        /// 1 to 12.
        month: u8,
        /// 1 to 5, 5 the last.
        week: u8,
        /// 0 (Sunday) to 6.
        weekday: u8,
    },
    /// `Jn`: day 1 to 365, February 29 not counted.
    JulianNoLeap(u16),
    /// `n`: day 0 to 365, February 29 counted.
    DayOfYear(u16),
}

/// A daylight saving time transition: its day, and the local time it happens at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Transition {
    /// The day.
    pub day: Day,
    /// Seconds after local midnight (POSIX allows -167 to 167 hours).
    pub time_s: i32,
}

/// A POSIX time zone. Offsets are POSIX's: seconds west of UTC (UTC = local + offset).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Zone {
    /// Standard time.
    pub std_offset_s: i32,
    /// Daylight saving time.
    pub dst_offset_s: i32,
    /// When daylight saving time starts (in standard time).
    pub dst_start: Transition,
    /// When it ends (in daylight saving time).
    pub dst_end: Transition,
}

const DAY: i64 = 86_400;

/// Days from 1970-01-01 to `year`-`month`-`day` (proleptic Gregorian).
const fn days_from_civil(year: i64, month: u8, day: u8) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The year of day `days` since 1970-01-01.
const fn year_of(days: i64) -> i64 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    yoe + era * 400 + if mp >= 10 { 1 } else { 0 }
}

const fn leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

const fn days_in_month(year: i64, month: u8) -> u8 {
    match month {
        2 if leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl Day {
    /// Days since 1970-01-01 of this day in `year`, if any.
    fn in_year(self, year: i64) -> Option<i64> {
        let jan1 = days_from_civil(year, 1, 1);
        match self {
            Day::None => None,
            Day::MonthWeekDay { month, week, weekday } => {
                let first = days_from_civil(year, month, 1);
                // 1970-01-01 was a Thursday (4).
                let first_weekday = (first + 4).rem_euclid(7);
                let mut day = 1 + (i64::from(weekday) - first_weekday).rem_euclid(7) + 7 * (i64::from(week) - 1);
                while day > i64::from(days_in_month(year, month)) {
                    day -= 7;
                }
                Some(first + day - 1)
            }
            Day::JulianNoLeap(n) => {
                let n = i64::from(n);
                Some(jan1 + n - 1 + i64::from(leap(year) && n >= 60))
            }
            Day::DayOfYear(n) => Some(jan1 + i64::from(n)),
        }
    }
}

impl Transition {
    fn to_words(self) -> [u32; 5] {
        let (kind, a, b, c) = match self.day {
            Day::None => (0, 0, 0, 0),
            Day::MonthWeekDay { month, week, weekday } => (1, month.into(), week.into(), weekday.into()),
            Day::JulianNoLeap(n) => (2, n.into(), 0, 0),
            Day::DayOfYear(n) => (3, n.into(), 0, 0),
        };
        [kind, a, b, c, self.time_s as u32]
    }

    fn from_words(w: [u32; 5]) -> Self {
        let day = match w[0] {
            1 => Day::MonthWeekDay { month: w[1] as u8, week: w[2] as u8, weekday: w[3] as u8 },
            2 => Day::JulianNoLeap(w[1] as u16),
            3 => Day::DayOfYear(w[1] as u16),
            _ => Day::None,
        };
        Self { day, time_s: w[4] as i32 }
    }
}

impl Zone {
    /// Words of [`to_words`](Self::to_words).
    pub const WORDS: usize = 12;

    /// As words, to keep in RTC memory.
    pub fn to_words(&self) -> [u32; Self::WORDS] {
        let mut w = [0; Self::WORDS];
        w[0] = self.std_offset_s as u32;
        w[1] = self.dst_offset_s as u32;
        w[2..7].copy_from_slice(&self.dst_start.to_words());
        w[7..12].copy_from_slice(&self.dst_end.to_words());
        w
    }

    /// From [`to_words`](Self::to_words).
    pub fn from_words(w: &[u32; Self::WORDS]) -> Self {
        let rule = |r: &[u32]| Transition::from_words(r.try_into().unwrap_or_default());
        Self {
            std_offset_s: w[0] as i32,
            dst_offset_s: w[1] as i32,
            dst_start: rule(&w[2..7]),
            dst_end: rule(&w[7..12]),
        }
    }

    /// UTC.
    pub const UTC: Zone = Zone {
        std_offset_s: 0,
        dst_offset_s: 0,
        dst_start: Transition { day: Day::None, time_s: 0 },
        dst_end: Transition { day: Day::None, time_s: 0 },
    };

    /// Whether daylight saving time is in effect at `utc`.
    pub fn is_dst(&self, utc: i64) -> bool {
        let year = year_of((utc - i64::from(self.std_offset_s)).div_euclid(DAY));
        let at = |t: Transition, offset: i32| {
            t.day.in_year(year).map(|day| day * DAY + i64::from(t.time_s) + i64::from(offset))
        };
        let (Some(start), Some(end)) = (at(self.dst_start, self.std_offset_s), at(self.dst_end, self.dst_offset_s))
        else {
            return false;
        };
        if start < end { (start..end).contains(&utc) } else { utc >= start || utc < end }
    }

    /// Seconds to add to UTC for local time at `utc`.
    pub fn offset(&self, utc: i64) -> i32 {
        -if self.is_dst(utc) { self.dst_offset_s } else { self.std_offset_s }
    }

    /// Local time at `utc`, as Unix seconds would count it.
    pub fn local(&self, utc: i64) -> i64 {
        utc + i64::from(self.offset(utc))
    }

    /// The first instant after `utc` when local time is `minute` minutes past midnight. A time
    /// skipped by a change to daylight saving time comes the next day.
    pub fn next(&self, utc: i64, minute: u16) -> i64 {
        let today = self.local(utc).div_euclid(DAY);
        for day in today..today + 3 {
            let local = day * DAY + i64::from(minute) * 60;
            // The hour back from daylight saving time comes twice: the first one.
            let first = [-self.std_offset_s, -self.dst_offset_s]
                .into_iter()
                .map(|offset| (local - i64::from(offset), offset))
                .filter(|&(candidate, offset)| candidate > utc && self.offset(candidate) == offset)
                .map(|(candidate, _)| candidate)
                .min();
            if let Some(at) = first {
                return at;
            }
        }
        // A day long gap: not a real zone.
        utc + DAY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Europe/Paris: `CET-1CEST,M3.5.0,M10.5.0/3`.
    const PARIS: Zone = Zone {
        std_offset_s: -3600,
        dst_offset_s: -7200,
        dst_start: Transition { day: Day::MonthWeekDay { month: 3, week: 5, weekday: 0 }, time_s: 7200 },
        dst_end: Transition { day: Day::MonthWeekDay { month: 10, week: 5, weekday: 0 }, time_s: 10_800 },
    };

    /// Australia/Sydney: `AEST-10AEDT,M10.1.0,M4.1.0/3` (summer at the end of the year).
    const SYDNEY: Zone = Zone {
        std_offset_s: -36_000,
        dst_offset_s: -39_600,
        dst_start: Transition { day: Day::MonthWeekDay { month: 10, week: 1, weekday: 0 }, time_s: 7200 },
        dst_end: Transition { day: Day::MonthWeekDay { month: 4, week: 1, weekday: 0 }, time_s: 10_800 },
    };

    fn utc(y: i64, mo: u8, d: u8, h: i64, mi: i64) -> i64 {
        days_from_civil(y, mo, d) * DAY + h * 3600 + mi * 60
    }

    #[test]
    fn civil() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 10, 24), 20_750);
        assert_eq!(year_of(20_750), 2026);
        assert_eq!(year_of(days_from_civil(2024, 12, 31)), 2024);
        assert_eq!(year_of(days_from_civil(2025, 1, 1)), 2025);
    }

    #[test]
    fn paris() {
        // 2026: summer time from March 29 01:00 UTC to October 25 01:00 UTC.
        assert_eq!(PARIS.offset(utc(2026, 3, 29, 0, 59)), 3600);
        assert_eq!(PARIS.offset(utc(2026, 3, 29, 1, 0)), 7200);
        assert_eq!(PARIS.offset(utc(2026, 10, 25, 0, 59)), 7200);
        assert_eq!(PARIS.offset(utc(2026, 10, 25, 1, 0)), 3600);
        assert_eq!(PARIS.local(utc(2026, 10, 24, 10, 0)), utc(2026, 10, 24, 12, 0));
    }

    #[test]
    fn sydney() {
        assert_eq!(SYDNEY.offset(utc(2026, 1, 15, 0, 0)), 39_600);
        assert_eq!(SYDNEY.offset(utc(2026, 7, 15, 0, 0)), 36_000);
    }

    #[test]
    fn next() {
        // 02:00 in Paris: tonight, or tomorrow once past.
        let at = 2 * 60;
        assert_eq!(PARIS.next(utc(2026, 10, 24, 10, 0), at), utc(2026, 10, 25, 0, 0), "02:00 CEST");
        assert_eq!(PARIS.next(utc(2026, 10, 25, 0, 0), at), utc(2026, 10, 25, 1, 0), "02:00 again, now CET");
        assert_eq!(PARIS.next(utc(2026, 10, 25, 1, 0), at), utc(2026, 10, 26, 1, 0));
        // 02:30 does not exist on March 29: the next day.
        assert_eq!(PARIS.next(utc(2026, 3, 28, 12, 0), 150), utc(2026, 3, 30, 0, 30));
        assert_eq!(Zone::UTC.next(utc(2026, 10, 24, 10, 0), 9 * 60), utc(2026, 10, 25, 9, 0));
    }

    #[test]
    fn words() {
        for zone in [PARIS, SYDNEY, Zone::UTC] {
            assert_eq!(Zone::from_words(&zone.to_words()), zone);
        }
    }

    #[test]
    fn rules() {
        assert_eq!(Day::JulianNoLeap(60).in_year(2024), Some(days_from_civil(2024, 3, 1)));
        assert_eq!(Day::DayOfYear(59).in_year(2024), Some(days_from_civil(2024, 2, 29)));
        // The last Sunday of February 2026 is the 22nd; the 5th week falls back to it.
        let last = Day::MonthWeekDay { month: 2, week: 5, weekday: 0 };
        assert_eq!(last.in_year(2026), Some(days_from_civil(2026, 2, 22)));
    }
}
