use time::{Date, Month, OffsetDateTime};

const DAILY_LOG_HALF_LIFE_DAYS: f64 = 30.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PathScoringSignals {
    pub(crate) memory_layer: &'static str,
    pub(crate) document_date: Option<String>,
    pub(crate) recency_multiplier: f64,
}

pub(crate) fn path_scoring_signals(path: &str) -> PathScoringSignals {
    path_scoring_signals_on(path, OffsetDateTime::now_utc().date())
}

fn path_scoring_signals_on(path: &str, today: Date) -> PathScoringSignals {
    if path == "MEMORY.md" {
        return PathScoringSignals {
            memory_layer: "curated",
            document_date: None,
            recency_multiplier: 1.0,
        };
    }

    let Some(document_date) = parse_daily_log_date(path) else {
        return PathScoringSignals {
            memory_layer: "workspace-note",
            document_date: None,
            recency_multiplier: 1.0,
        };
    };

    PathScoringSignals {
        memory_layer: "daily-log",
        document_date: Some(document_date.to_string()),
        recency_multiplier: daily_log_recency_multiplier(document_date, today),
    }
}

fn parse_daily_log_date(path: &str) -> Option<Date> {
    let name = path.rsplit('/').next()?.strip_suffix(".md")?;
    let mut parts = name.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u8>().ok()?;
    let day = parts.next()?.parse::<u8>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()
}

fn daily_log_recency_multiplier(document_date: Date, today: Date) -> f64 {
    let age_days = (today - document_date).whole_days().max(0) as f64;
    2f64.powf(-(age_days / DAILY_LOG_HALF_LIFE_DAYS))
    // Equivalent to exp(-ln(2) * age / half_life) and matches the
    // "recent daily logs win, durable facts belong in MEMORY.md" pattern used
    // by mature coding-agent memory systems.
}

#[cfg(test)]
mod tests {
    use super::{PathScoringSignals, path_scoring_signals_on};
    use time::{Date, Month};

    #[test]
    fn curated_memory_has_no_decay() {
        let today = Date::from_calendar_date(2026, Month::March, 26).unwrap();

        assert_eq!(
            path_scoring_signals_on("MEMORY.md", today),
            PathScoringSignals {
                memory_layer: "curated",
                document_date: None,
                recency_multiplier: 1.0,
            }
        );
    }

    #[test]
    fn daily_logs_decay_by_age() {
        let today = Date::from_calendar_date(2026, Month::March, 26).unwrap();
        let recent = path_scoring_signals_on("memory/2026-03-26.md", today);
        let week_old = path_scoring_signals_on("memory/2026-03-19.md", today);
        let stale = path_scoring_signals_on("memory/2025-10-29.md", today);

        assert_eq!(recent.memory_layer, "daily-log");
        assert_eq!(recent.document_date.as_deref(), Some("2026-03-26"));
        assert!((recent.recency_multiplier - 1.0).abs() < f64::EPSILON);
        assert!((week_old.recency_multiplier - 0.850667).abs() < 0.001);
        assert!(stale.recency_multiplier < 0.05);
    }

    #[test]
    fn non_daily_workspace_notes_stay_neutral() {
        let today = Date::from_calendar_date(2026, Month::March, 26).unwrap();
        let signals = path_scoring_signals_on("memory/network.md", today);

        assert_eq!(
            signals,
            PathScoringSignals {
                memory_layer: "workspace-note",
                document_date: None,
                recency_multiplier: 1.0,
            }
        );
    }
}
