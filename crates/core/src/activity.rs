//! Time spent coding, per day, project and language. Kept on this computer only.
//!
//! Saved as one line per day, project and language: `2026-10-14\tdemo\tRust\t3600`.

use std::collections::BTreeMap;

/// Days kept; older ones are dropped.
pub const KEPT_DAYS: usize = 400;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// Date `YYYY-MM-DD` → (project, language) → seconds.
    days: BTreeMap<String, BTreeMap<(String, String), u64>>,
}

impl Activity {
    pub fn parse(text: &str) -> Self {
        let mut activity = Self::default();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            if let [date, project, language, seconds] = fields[..]
                && let Ok(seconds) = seconds.parse()
            {
                activity.add(date, project, language, seconds);
            }
        }
        activity
    }

    pub fn to_text(&self) -> String {
        let mut text = String::new();
        for (date, buckets) in &self.days {
            for ((project, language), seconds) in buckets {
                text.push_str(&format!("{date}\t{project}\t{language}\t{seconds}\n"));
            }
        }
        text
    }

    pub fn add(&mut self, date: &str, project: &str, language: &str, seconds: u64) {
        let clean = |text: &str| text.replace(['\t', '\n', '\r'], " ");
        let key = (clean(project), clean(language));
        *self
            .days
            .entry(date.to_owned())
            .or_default()
            .entry(key)
            .or_default() += seconds;
        while self.days.len() > KEPT_DAYS {
            self.days.pop_first();
        }
    }

    pub fn day_total(&self, date: &str) -> u64 {
        self.days
            .get(date)
            .map_or(0, |buckets| buckets.values().sum())
    }

    /// Each of these dates with its total, in the same order.
    pub fn totals(&self, dates: &[String]) -> Vec<(String, u64)> {
        dates
            .iter()
            .map(|date| (date.clone(), self.day_total(date)))
            .collect()
    }

    fn grouped(
        &self,
        dates: &[String],
        key: impl Fn(&(String, String)) -> String,
    ) -> Vec<(String, u64)> {
        let mut totals: BTreeMap<String, u64> = BTreeMap::new();
        for date in dates {
            for (bucket, seconds) in self.days.get(date).into_iter().flatten() {
                *totals.entry(key(bucket)).or_default() += seconds;
            }
        }
        let mut list: Vec<(String, u64)> = totals.into_iter().collect();
        list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        list
    }

    pub fn by_project(&self, dates: &[String]) -> Vec<(String, u64)> {
        self.grouped(dates, |(project, _)| project.clone())
    }

    pub fn by_language(&self, dates: &[String]) -> Vec<(String, u64)> {
        self.grouped(dates, |(_, language)| language.clone())
    }
}

/// `3h 05m`, `12m`, `40s`.
pub fn duration(seconds: u64) -> String {
    let (hours, minutes) = (seconds / 3600, seconds / 60 % 60);
    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, _) => format!("{minutes}m"),
        _ => format!("{hours}h {minutes:02}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_up_and_round_trips() {
        let mut activity = Activity::default();
        activity.add("2026-10-13", "demo", "Rust", 600);
        activity.add("2026-10-14", "demo", "Rust", 1200);
        activity.add("2026-10-14", "demo", "Rust", 300);
        activity.add("2026-10-14", "site\tweb", "CSS", 60);
        assert_eq!(activity.day_total("2026-10-14"), 1560);
        let again = Activity::parse(&activity.to_text());
        assert_eq!(again, activity);
        let dates = vec!["2026-10-13".to_owned(), "2026-10-14".to_owned()];
        assert_eq!(activity.by_project(&dates)[0], ("demo".to_owned(), 2100));
        assert_eq!(activity.by_language(&dates)[1], ("CSS".to_owned(), 60));
        assert_eq!(activity.totals(&dates)[0].1, 600);
    }

    #[test]
    fn keeps_a_limit_of_days() {
        let mut activity = Activity::default();
        for day in 0..KEPT_DAYS + 5 {
            activity.add(&format!("d{day:04}"), "p", "l", 1);
        }
        assert_eq!(activity.days.len(), KEPT_DAYS);
        assert_eq!(activity.day_total("d0000"), 0);
    }

    #[test]
    fn durations() {
        assert_eq!(duration(40), "40s");
        assert_eq!(duration(720), "12m");
        assert_eq!(duration(11_100), "3h 05m");
    }
}
