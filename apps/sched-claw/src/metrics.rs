use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricGoal {
    Minimize,
    Maximize,
}

impl MetricGoal {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimize => "minimize",
            Self::Maximize => "maximize",
        }
    }

    #[must_use]
    pub fn improvement_pct(self, baseline: f64, candidate: f64) -> Option<f64> {
        if !baseline.is_finite() || !candidate.is_finite() {
            return None;
        }
        if baseline == 0.0 {
            return if candidate == 0.0 { Some(0.0) } else { None };
        }
        let ratio = match self {
            Self::Minimize => (baseline - candidate) / baseline,
            Self::Maximize => (candidate - baseline) / baseline,
        };
        Some(ratio * 100.0)
    }
}

impl std::str::FromStr for MetricGoal {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "min" | "minimize" | "lower" | "lower_is_better" => Ok(Self::Minimize),
            "max" | "maximize" | "higher" | "higher_is_better" => Ok(Self::Maximize),
            other => bail!("unsupported metric goal `{other}`"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetricTarget {
    pub name: String,
    pub goal: MetricGoal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guardrail {
    pub name: String,
    pub goal: MetricGoal,
    pub max_regression_pct: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

pub type MetricMap = BTreeMap<String, f64>;

pub fn parse_metric_assignment(value: &str) -> Result<(String, f64)> {
    let (name, raw_value) = value
        .split_once('=')
        .ok_or_else(|| anyhow!("expected NAME=VALUE, got `{value}`"))?;
    let name = name.trim();
    if name.is_empty() {
        bail!("metric name cannot be empty");
    }
    let parsed = raw_value
        .trim()
        .parse::<f64>()
        .with_context(|| format!("invalid metric value `{raw_value}`"))?;
    if !parsed.is_finite() {
        bail!("metric value must be finite");
    }
    Ok((name.to_string(), parsed))
}

pub fn parse_guardrail(value: &str) -> Result<Guardrail> {
    let mut parts = value.split(':');
    let name = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("guardrail must include a metric name"))?;
    let goal = parts
        .next()
        .ok_or_else(|| anyhow!("guardrail must include a goal"))?
        .parse::<MetricGoal>()?;
    let max_regression_pct = parts
        .next()
        .ok_or_else(|| anyhow!("guardrail must include max regression percent"))?
        .trim()
        .parse::<f64>()
        .with_context(|| format!("invalid guardrail regression percent in `{value}`"))?;
    if !max_regression_pct.is_finite() || max_regression_pct < 0.0 {
        bail!("guardrail max regression percent must be a finite non-negative number");
    }
    if parts.next().is_some() {
        bail!("guardrail must use NAME:GOAL:MAX_REGRESSION_PCT, got extra fields in `{value}`");
    }
    Ok(Guardrail {
        name: name.to_string(),
        goal,
        max_regression_pct,
        notes: None,
    })
}

pub fn median_metric<'a>(
    metric_name: &str,
    metrics: impl IntoIterator<Item = &'a MetricMap>,
) -> Option<f64> {
    let mut values = metrics
        .into_iter()
        .filter_map(|run| run.get(metric_name).copied())
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }
    values.sort_by(|left, right| left.total_cmp(right));
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        Some(values[mid])
    } else {
        Some((values[mid - 1] + values[mid]) / 2.0)
    }
}

trait ErrorContextExt<T> {
    fn with_context(self, context: impl FnOnce() -> String) -> Result<T>;
}

impl<T, E> ErrorContextExt<T> for std::result::Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn with_context(self, context: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|error| anyhow!("{}: {error}", context()))
    }
}

#[cfg(test)]
mod tests {
    use super::{MetricGoal, median_metric, parse_guardrail, parse_metric_assignment};
    use std::collections::BTreeMap;

    #[test]
    fn metric_goal_improvement_pct_respects_direction() {
        assert_eq!(MetricGoal::Minimize.improvement_pct(10.0, 8.0), Some(20.0));
        assert_eq!(
            MetricGoal::Maximize.improvement_pct(100.0, 110.0),
            Some(10.0)
        );
        assert_eq!(
            MetricGoal::Maximize.improvement_pct(100.0, 90.0),
            Some(-10.0)
        );
    }

    #[test]
    fn parses_metric_assignment() {
        assert_eq!(
            parse_metric_assignment("latency_ms=12.5").unwrap(),
            ("latency_ms".to_string(), 12.5)
        );
    }

    #[test]
    fn parses_guardrail() {
        let guardrail = parse_guardrail("throughput:maximize:5").unwrap();
        assert_eq!(guardrail.name, "throughput");
        assert_eq!(guardrail.goal, MetricGoal::Maximize);
        assert_eq!(guardrail.max_regression_pct, 5.0);
    }

    #[test]
    fn median_metric_uses_sorted_middle() {
        let runs = vec![
            BTreeMap::from([(String::from("latency_ms"), 12.0)]),
            BTreeMap::from([(String::from("latency_ms"), 8.0)]),
            BTreeMap::from([(String::from("latency_ms"), 10.0)]),
        ];
        assert_eq!(median_metric("latency_ms", runs.iter()), Some(10.0));
    }
}
