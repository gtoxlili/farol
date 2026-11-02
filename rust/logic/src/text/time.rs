//! Clock and duration text. Times are Unix epoch milliseconds from the
//! plugin's clock; `tz_min` is the display time zone's offset east of UTC.

use alloc::format;
use alloc::string::String;

use crate::lang::Lang;
use crate::tr;

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
const WEEKDAYS_EN: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
const MONTHS_EN: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "11 月 2 日 星期一", "Mon, Nov 2".
pub fn date(epoch_ms: i64, tz_min: i32, lang: Lang) -> String {
    let days = local(epoch_ms, tz_min).div_euclid(DAY);
    let (_, m, d) = civil(days);
    let weekday = days.rem_euclid(7) as usize;
    match lang {
        Lang::Zh => format!("{m} 月 {d} 日 {}", WEEKDAYS[weekday]),
        Lang::En => format!(
            "{}, {} {d}",
            WEEKDAYS_EN[weekday],
            MONTHS_EN[m as usize - 1]
        ),
    }
}

/// "刚刚", "4 分钟前"; "just now", "4 min ago", "2 h ago", "3 d ago".
pub fn ago(now_ms: i64, then_ms: i64, lang: Lang) -> String {
    let dt = (now_ms - then_ms).max(0);
    if dt < MINUTE {
        String::from(lang.pick("刚刚", "just now"))
    } else if dt < HOUR {
        tr!(lang, "{} 分钟前", "{} min ago", dt / MINUTE)
    } else if dt < DAY {
        tr!(lang, "{} 小时前", "{} h ago", dt / HOUR)
    } else {
        tr!(lang, "{} 天前", "{} d ago", dt / DAY)
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

/// "6 分钟", "1 小时 5 分钟"; "6 min", "1 h 5 min".
pub fn span(ms: i64, lang: Lang) -> String {
    let m = ms.max(0) / MINUTE;
    if m < 1 {
        String::from(lang.pick("不到 1 分钟", "under 1 min"))
    } else if m < 60 {
        tr!(lang, "{} 分钟", "{} min", m)
    } else if m % 60 == 0 {
        tr!(lang, "{} 小时", "{} h", m / 60)
    } else {
        tr!(lang, "{} 小时 {} 分钟", "{} h {} min", m / 60, m % 60)
    }
}

/// When a quota window resets: "1:12 后重置" within a day, "2 天 4 小时"
/// within a week, else "周四 08:00" (and the English equivalents).
pub fn reset(now_ms: i64, at_ms: i64, tz_min: i32, lang: Lang) -> String {
    let dt = at_ms - now_ms;
    if dt <= 0 {
        String::from(lang.pick("已重置", "reset"))
    } else if dt < DAY {
        tr!(
            lang,
            "{}:{:02} 后重置",
            "resets in {}:{:02}",
            dt / HOUR,
            dt % HOUR / MINUTE
        )
    } else if dt < 7 * DAY {
        tr!(
            lang,
            "{} 天 {} 小时",
            "{} d {} h",
            dt / DAY,
            dt % DAY / HOUR
        )
    } else {
        let days = local(at_ms, tz_min).div_euclid(DAY).rem_euclid(7) as usize;
        let day = lang.pick(WEEKDAYS_SHORT[days], WEEKDAYS_EN[days]);
        format!("{day} {}", clock(at_ms, tz_min))
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
        assert_eq!(date(T, 480, Lang::Zh), "11 月 2 日 星期一");
    }

    #[test]
    fn ages_round_down() {
        assert_eq!(ago(T, T - 30_000, Lang::Zh), "刚刚");
        assert_eq!(ago(T, T - 4 * MINUTE - 59_000, Lang::Zh), "4 分钟前");
        assert_eq!(ago(T, T - 3 * HOUR, Lang::Zh), "3 小时前");
        assert_eq!(ago(T, T - 50 * HOUR, Lang::Zh), "2 天前");
        assert_eq!(ago(T, T + 5_000, Lang::Zh), "刚刚");
    }

    #[test]
    fn stopwatch_and_span() {
        assert_eq!(stopwatch(768_000), "12:48");
        assert_eq!(stopwatch(3_723_000), "1:02:03");
        assert_eq!(span(6 * MINUTE + 10_000, Lang::Zh), "6 分钟");
        assert_eq!(span(65 * MINUTE, Lang::Zh), "1 小时 5 分钟");
    }

    #[test]
    fn resets_scale_with_distance() {
        assert_eq!(
            reset(T, T + HOUR + 12 * MINUTE, 480, Lang::Zh),
            "1:12 后重置"
        );
        assert_eq!(
            reset(T, T + 2 * DAY + 4 * HOUR, 480, Lang::Zh),
            "2 天 4 小时"
        );
        assert_eq!(reset(T, T + 9 * DAY, 480, Lang::Zh), "周三 09:41");
        assert_eq!(reset(T, T - 1, 480, Lang::Zh), "已重置");
    }

    #[test]
    fn english_times() {
        assert_eq!(date(T, 480, Lang::En), "Mon, Nov 2");
        assert_eq!(ago(T, T - 3 * HOUR, Lang::En), "3 h ago");
        assert_eq!(span(65 * MINUTE, Lang::En), "1 h 5 min");
        assert_eq!(
            reset(T, T + HOUR + 12 * MINUTE, 480, Lang::En),
            "resets in 1:12"
        );
        assert_eq!(reset(T, T + 9 * DAY, 480, Lang::En), "Wed 09:41");
    }
}
