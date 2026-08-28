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
    pub elapse_seconds: Vec<f64>,
    pub slice_quantity: Vec<f64>,
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

fn schema_version() -> u32 {
    1
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
