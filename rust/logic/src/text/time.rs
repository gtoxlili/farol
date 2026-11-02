//! Clock and duration text. Times are Unix epoch milliseconds from the
//! plugin's clock; `tz_min` is the display time zone's offset east of UTC.

use alloc::format;
use alloc::string::String;

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// Days since 1970-01-01 to (year, month 1..=12, day 1..=31); Hinnant's
/// civil-from-days.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn local(epoch_ms: i64, tz_min: i32) -> i64 {
    epoch_ms + i64::from(tz_min) * MINUTE
}

/// "09:41".
pub fn clock(epoch_ms: i64, tz_min: i32) -> String {
    let t = local(epoch_ms, tz_min).rem_euclid(DAY);
    format!("{:02}:{:02}", t / HOUR, t % HOUR / MINUTE)
}

const WEEKDAYS: [&str; 7] = [
    "星期四",
    "星期五",
    "星期六",
    "星期日",
    "星期一",
    "星期二",
    "星期三",
];
const WEEKDAYS_SHORT: [&str; 7] = ["周四", "周五", "周六", "周日", "周一", "周二", "周三"];

/// "11 月 2 日 星期日".
pub fn date(epoch_ms: i64, tz_min: i32) -> String {
    let days = local(epoch_ms, tz_min).div_euclid(DAY);
    let (_, m, d) = civil(days);
    format!("{m} 月 {d} 日 {}", WEEKDAYS[days.rem_euclid(7) as usize])
}

/// "刚刚", "4 分钟前", "2 小时前", "3 天前".
pub fn ago(now_ms: i64, then_ms: i64) -> String {
    let dt = (now_ms - then_ms).max(0);
    if dt < MINUTE {
        String::from("刚刚")
    } else if dt < HOUR {
        format!("{} 分钟前", dt / MINUTE)
    } else if dt < DAY {
        format!("{} 小时前", dt / HOUR)
    } else {
        format!("{} 天前", dt / DAY)
    }
}

/// A stopwatch: "12:48", or "1:02:03" past an hour.
pub fn stopwatch(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// "6 分钟", "1 小时 5 分钟".
pub fn span(ms: i64) -> String {
    let m = ms.max(0) / MINUTE;
    if m < 1 {
        String::from("不到 1 分钟")
    } else if m < 60 {
        format!("{m} 分钟")
    } else if m % 60 == 0 {
        format!("{} 小时", m / 60)
    } else {
        format!("{} 小时 {} 分钟", m / 60, m % 60)
    }
}

/// When a quota window resets: "1:12 后重置" within a day, "2 天 4 小时"
/// within a week, else "周四 08:00".
pub fn reset(now_ms: i64, at_ms: i64, tz_min: i32) -> String {
    let dt = at_ms - now_ms;
    if dt <= 0 {
        String::from("已重置")
    } else if dt < DAY {
        format!("{}:{:02} 后重置", dt / HOUR, dt % HOUR / MINUTE)
    } else if dt < 7 * DAY {
        format!("{} 天 {} 小时", dt / DAY, dt % DAY / HOUR)
    } else {
        let days = local(at_ms, tz_min).div_euclid(DAY);
        format!(
            "{} {}",
            WEEKDAYS_SHORT[days.rem_euclid(7) as usize],
            clock(at_ms, tz_min)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-11-02 09:41:00 in UTC+8 is 01:41:00 UTC.
    const T: i64 = 1_793_583_660_000;

    #[test]
    fn clock_and_date_follow_the_offset() {
        assert_eq!(clock(T, 480), "09:41");
        assert_eq!(clock(T, -480), "17:41");
        assert_eq!(date(T, 480), "11 月 2 日 星期一");
    }

    #[test]
    fn ages_round_down() {
        assert_eq!(ago(T, T - 30_000), "刚刚");
        assert_eq!(ago(T, T - 4 * MINUTE - 59_000), "4 分钟前");
        assert_eq!(ago(T, T - 3 * HOUR), "3 小时前");
        assert_eq!(ago(T, T - 50 * HOUR), "2 天前");
        assert_eq!(ago(T, T + 5_000), "刚刚");
    }

    #[test]
    fn stopwatch_and_span() {
        assert_eq!(stopwatch(768_000), "12:48");
        assert_eq!(stopwatch(3_723_000), "1:02:03");
        assert_eq!(span(6 * MINUTE + 10_000), "6 分钟");
        assert_eq!(span(65 * MINUTE), "1 小时 5 分钟");
    }

    #[test]
    fn resets_scale_with_distance() {
        assert_eq!(reset(T, T + HOUR + 12 * MINUTE, 480), "1:12 后重置");
        assert_eq!(reset(T, T + 2 * DAY + 4 * HOUR, 480), "2 天 4 小时");
        assert_eq!(reset(T, T + 9 * DAY, 480), "周三 09:41");
        assert_eq!(reset(T, T - 1, 480), "已重置");
    }
}
