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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformancePreference {
    LatencyThroughput,
    ThroughputLatency,
    LatencyOnly,
    ThroughputOnly,
    Custom,
}

impl PerformancePreference {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LatencyThroughput => "latency_throughput",
            Self::ThroughputLatency => "throughput_latency",
            Self::LatencyOnly => "latency_only",
            Self::ThroughputOnly => "throughput_only",
            Self::Custom => "custom",
        }
    }
}

impl Default for PerformancePreference {
    fn default() -> Self {
        Self::Custom
    }
}

impl std::str::FromStr for PerformancePreference {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "latency-throughput" | "latency_throughput" | "latency_first" => {
                Ok(Self::LatencyThroughput)
            }
            "throughput-latency" | "throughput_latency" | "throughput_first" => {
                Ok(Self::ThroughputLatency)
            }
            "latency" | "latency_only" => Ok(Self::LatencyOnly),
            "throughput" | "throughput_only" => Ok(Self::ThroughputOnly),
            "custom" => Ok(Self::Custom),
            other => bail!("unsupported performance preference `{other}`"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementBasis {
    Direct,
    ProxyEstimate,
}

impl MeasurementBasis {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::ProxyEstimate => "proxy_estimate",
        }
    }
}

impl Default for MeasurementBasis {
    fn default() -> Self {
        Self::Direct
    }
}

impl std::str::FromStr for MeasurementBasis {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "direct" => Ok(Self::Direct),
            "proxy" | "proxy_estimate" | "proxy-estimate" => Ok(Self::ProxyEstimate),
            other => bail!("unsupported measurement basis `{other}`"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PerformancePolicy {
    #[serde(default)]
    pub preference: PerformancePreference,
    #[serde(default)]
    pub basis: MeasurementBasis,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proxy_metrics: Vec<MetricTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl PerformancePolicy {
    #[must_use]
    pub fn summary(&self) -> String {
        let mut summary = format!("{} / {}", self.preference.as_str(), self.basis.as_str());
        if !self.proxy_metrics.is_empty() {
            summary.push_str(" / proxies=");
            summary.push_str(
                &self
                    .proxy_metrics
                    .iter()
                    .map(metric_target_summary)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        summary
    }

    pub fn validate(&self) -> Result<()> {
        if self.basis == MeasurementBasis::ProxyEstimate && self.proxy_metrics.is_empty() {
            bail!("proxy_estimate basis requires at least one proxy metric");
        }
        Ok(())
    }
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

pub fn parse_metric_target(value: &str) -> Result<MetricTarget> {
    let mut parts = value.split(':');
    let name = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("metric target must include a metric name"))?;
    let goal = parts
        .next()
        .ok_or_else(|| anyhow!("metric target must include a goal"))?
        .parse::<MetricGoal>()?;
    let unit = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    if parts.next().is_some() {
        bail!("metric target must use NAME:GOAL[:UNIT], got extra fields in `{value}`");
    }
    Ok(MetricTarget {
        name: name.to_string(),
        goal,
        unit,
        notes: None,
    })
}

#[must_use]
pub fn infer_performance_policy(
    primary_metric: &MetricTarget,
    guardrails: &[Guardrail],
    proxy_metrics: Vec<MetricTarget>,
) -> PerformancePolicy {
    let primary_name = primary_metric.name.to_ascii_lowercase();
    let guardrail_names = guardrails
        .iter()
        .map(|guardrail| guardrail.name.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let basis = if proxy_metrics.is_empty() {
        MeasurementBasis::Direct
    } else if is_direct_performance_metric(&primary_name)
        || guardrail_names
            .iter()
            .any(|metric_name| is_direct_performance_metric(metric_name))
    {
        MeasurementBasis::Direct
    } else {
        MeasurementBasis::ProxyEstimate
    };
    let preference = infer_preference(&primary_name, &guardrail_names);
    PerformancePolicy {
        preference,
        basis,
        proxy_metrics,
        notes: None,
    }
}

pub fn median_metric<'a>(
    metric_name: &str,
    metrics: impl IntoIterator<Item = &'a MetricMap>,
) -> Option<f64> {
    let mut values = metric_values(metric_name, metrics);
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

pub fn relative_spread_pct<'a>(
    metric_name: &str,
    metrics: impl IntoIterator<Item = &'a MetricMap>,
) -> Option<f64> {
    let mut values = metric_values(metric_name, metrics);
    if values.is_empty() {
        return None;
    }
    if values.len() == 1 {
        return Some(0.0);
    }
    values.sort_by(|left, right| left.total_cmp(right));
    let min = values.first().copied().unwrap_or_default();
    let max = values.last().copied().unwrap_or_default();
    let median = median_from_sorted(&values);
    if median == 0.0 {
        return if min == max { Some(0.0) } else { None };
    }
    Some(((max - min).abs() / median.abs()) * 100.0)
}

pub fn metric_values<'a>(
    metric_name: &str,
    metrics: impl IntoIterator<Item = &'a MetricMap>,
) -> Vec<f64> {
    metrics
        .into_iter()
        .filter_map(|run| run.get(metric_name).copied())
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>()
}

fn median_from_sorted(values: &[f64]) -> f64 {
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2.0
    }
}

fn infer_preference(primary_name: &str, guardrail_names: &[String]) -> PerformancePreference {
    let primary_is_latency = is_latency_metric(primary_name);
    let primary_is_throughput = is_throughput_metric(primary_name);
    let has_latency_guardrail = guardrail_names
        .iter()
        .any(|metric_name| is_latency_metric(metric_name));
    let has_throughput_guardrail = guardrail_names
        .iter()
        .any(|metric_name| is_throughput_metric(metric_name));
    if primary_is_latency && has_throughput_guardrail {
        PerformancePreference::LatencyThroughput
    } else if primary_is_throughput && has_latency_guardrail {
        PerformancePreference::ThroughputLatency
    } else if primary_is_latency {
        PerformancePreference::LatencyOnly
    } else if primary_is_throughput {
        PerformancePreference::ThroughputOnly
    } else {
        PerformancePreference::Custom
    }
}

fn is_direct_performance_metric(metric_name: &str) -> bool {
    is_latency_metric(metric_name) || is_throughput_metric(metric_name)
}

fn is_latency_metric(metric_name: &str) -> bool {
    matches_metric(
        metric_name,
        &["latency", "tail", "p99", "p95", "jitter", "response"],
    )
}

fn is_throughput_metric(metric_name: &str) -> bool {
    matches_metric(
        metric_name,
        &["throughput", "qps", "tps", "ops", "bandwidth", "rps"],
    )
}

fn matches_metric(metric_name: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| metric_name.contains(needle))
}

fn metric_target_summary(metric: &MetricTarget) -> String {
    match &metric.unit {
        Some(unit) => format!("{}:{}:{}", metric.name, metric.goal.as_str(), unit),
        None => format!("{}:{}", metric.name, metric.goal.as_str()),
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
    use super::{
        MeasurementBasis, MetricGoal, PerformancePreference, infer_performance_policy,
        median_metric, parse_guardrail, parse_metric_assignment, parse_metric_target,
        relative_spread_pct,
    };
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

    #[test]
    fn relative_spread_pct_uses_range_over_median() {
        let runs = vec![
            BTreeMap::from([(String::from("latency_ms"), 8.0)]),
            BTreeMap::from([(String::from("latency_ms"), 10.0)]),
            BTreeMap::from([(String::from("latency_ms"), 12.0)]),
        ];
        assert_eq!(relative_spread_pct("latency_ms", runs.iter()), Some(40.0));
    }

    #[test]
    fn parses_metric_target() {
        let target = parse_metric_target("ipc:maximize").unwrap();
        assert_eq!(target.name, "ipc");
        assert_eq!(target.goal, MetricGoal::Maximize);
        assert_eq!(target.unit, None);
    }

    #[test]
    fn infers_latency_throughput_policy() {
        let policy = infer_performance_policy(
            &parse_metric_target("latency_ms:minimize").unwrap(),
            &[parse_guardrail("throughput:maximize:5").unwrap()],
            Vec::new(),
        );
        assert_eq!(policy.preference, PerformancePreference::LatencyThroughput);
        assert_eq!(policy.basis, MeasurementBasis::Direct);
    }

    #[test]
    fn infers_proxy_policy_when_only_ipc_cpi_exist() {
        let policy = infer_performance_policy(
            &parse_metric_target("ipc:maximize").unwrap(),
            &[parse_guardrail("cpi:minimize:3").unwrap()],
            vec![
                parse_metric_target("ipc:maximize").unwrap(),
                parse_metric_target("cpi:minimize").unwrap(),
            ],
        );
        assert_eq!(policy.preference, PerformancePreference::Custom);
        assert_eq!(policy.basis, MeasurementBasis::ProxyEstimate);
    }
}
