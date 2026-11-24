//! UTC calendar fields for the two date strings upstream builds from `Date`: the logs folder name and `toUTCString`.

use std::time::SystemTime;

pub struct Utc {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// 0 is Sunday.
    pub weekday: u32,
}

pub fn utc(time: SystemTime) -> Utc {
    let seconds = time.duration_since(SystemTime::UNIX_EPOCH).expect("the clock is after 1970").as_secs() as i64;
    let (days, of_day) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400) as u32);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 } as u32;
    Utc {
        year: year_of_era + era * 400 + i64::from(month <= 2),
        month,
        day: (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32,
        hour: of_day / 3600,
        minute: of_day / 60 % 60,
        second: of_day % 60,
        weekday: (days + 4).rem_euclid(7) as u32,
    }
}

/// `new Date().toISOString().replace(/-|:|\.\d+Z$/g, '')`, the name of a session's logs folder.
pub fn logs_folder_name(time: SystemTime) -> String {
    let t = utc(time);
    format!("{:04}{:02}{:02}T{:02}{:02}{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

/// `Date.prototype.toUTCString`.
pub fn utc_string(time: SystemTime) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let t = utc(time);
    format!("{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT", DAYS[t.weekday as usize], t.day, MONTHS[t.month as usize - 1], t.year, t.hour, t.minute, t.second)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_like_javascript_dates() {
        // new Date(1795523940000): 2026-11-24T12:39:00.000Z
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_523_940);
        assert_eq!(logs_folder_name(time), "20261124T123900");
        assert_eq!(utc_string(time), "Tue, 24 Nov 2026 12:39:00 GMT");
        assert_eq!(utc_string(SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400)), "Tue, 29 Feb 2000 00:00:00 GMT");
    }
}
