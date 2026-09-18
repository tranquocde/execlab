use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn initial_position(&self, quantity: f64) -> f64 {
        match self {
            Self::Buy => -quantity,
            Self::Sell => quantity,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderSpec {
    pub symbol: String,
    pub side: Side,
    pub quantity: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TwapInputs {
    #[serde(default = "default_start_times")]
    pub start_times: Vec<String>,
    #[serde(default = "default_time_taken_seconds")]
    pub time_taken_seconds: Vec<f64>,
    #[serde(default = "default_trade_frequency_seconds")]
    pub trade_frequency_seconds: Vec<f64>,
}

fn default_start_times() -> Vec<String> {
    vec!["10:00:00".into()]
}

fn default_time_taken_seconds() -> Vec<f64> {
    vec![1_800.0]
}

fn default_trade_frequency_seconds() -> Vec<f64> {
    vec![60.0]
}

pub fn parse_start_time(value: &str) -> Result<i64, String> {
    let parts: Vec<_> = value.trim().split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return Err(format!("invalid start time {value:?}; expected HH:MM[:SS]"));
    }
    let parse = |part: &str| {
        part.parse::<i64>()
            .map_err(|_| format!("invalid start time {value:?}; expected HH:MM[:SS]"))
    };
    let hour = parse(parts[0])?;
    let minute = parse(parts[1])?;
    let second = if parts.len() == 3 {
        parse(parts[2])?
    } else {
        0
    };
    if hour < 0 || minute < 0 || second < 0 || hour >= 24 || minute >= 60 || second >= 60 {
        return Err(format!("invalid start time {value:?}; expected HH:MM[:SS]"));
    }
    Ok(hour * 3_600 + minute * 60 + second)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategySpec {
    pub name: String,
    pub inputs: TwapInputs,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoricalSelection {
    pub mode: String,
}

impl Default for HistoricalSelection {
    fn default() -> Self {
        Self {
            mode: "all_available".into(),
        }
    }
}

fn schema_version() -> u32 {
    3
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scenario {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub order: OrderSpec,
    pub strategies: Vec<StrategySpec>,
    #[serde(default)]
    pub historical_selection: HistoricalSelection,
    pub latest_successful_run_id: Option<String>,
    pub last_attempt_run_id: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ScenarioDraft {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub order: OrderSpec,
    pub strategies: Vec<StrategySpec>,
    #[serde(default)]
    pub historical_selection: HistoricalSelection,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    pub schema_version: u32,
    pub id: String,
    pub scenario_id: String,
    pub status: RunStatus,
    pub scenario_snapshot: Scenario,
    pub resolved_intervals: Vec<String>,
    pub params: serde_json::Value,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
    pub pid: Option<u32>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct IntervalFile {
    pub id: String,
    pub file: String,
    pub path: String,
    pub status: &'static str,
}

#[cfg(test)]
mod tests {
    use super::parse_start_time;

    #[test]
    fn parses_ict_wall_clock_times() {
        assert_eq!(parse_start_time("10:30").unwrap(), 37_800);
        assert_eq!(parse_start_time("10:30:15").unwrap(), 37_815);
    }

    #[test]
    fn rejects_out_of_range_wall_clock_times() {
        assert!(parse_start_time("24:00").is_err());
        assert!(parse_start_time("10:60:00").is_err());
        assert!(parse_start_time("10").is_err());
    }
}
