use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use uuid::Uuid;

use crate::model::{parse_start_time, RunRecord, Scenario, ScenarioDraft};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn atomic_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path.parent().ok_or("path has no parent")?;
    fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let temp = path.with_extension("tmp");
    fs::write(
        &temp,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("cannot write {}: {e}", temp.display()))?;
    fs::rename(&temp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
}

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(root.join("scenarios")).map_err(|e| e.to_string())?;
        fs::create_dir_all(root.join("runs")).map_err(|e| e.to_string())?;
        Ok(Self { root })
    }

    fn scenario_path(&self, id: &str) -> PathBuf {
        self.root.join("scenarios").join(id).join("scenario.json")
    }
    pub fn run_dir(&self, scenario: &str, run: &str) -> PathBuf {
        self.root.join("runs").join(scenario).join(run)
    }

    pub fn list(&self) -> Vec<Scenario> {
        let mut result: Vec<_> = fs::read_dir(self.root.join("scenarios"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| fs::read_to_string(entry.path().join("scenario.json")).ok())
            .filter_map(|text| serde_json::from_str(&text).ok())
            .collect();
        result.sort_by(|a: &Scenario, b: &Scenario| b.updated_at.cmp(&a.updated_at));
        result
    }

    pub fn get(&self, id: &str) -> Result<Scenario, String> {
        let path = self.scenario_path(id);
        serde_json::from_str(
            &fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?,
        )
        .map_err(|e| format!("cannot parse {}: {e}", path.display()))
    }

    pub fn create(&self, draft: ScenarioDraft) -> Result<Scenario, String> {
        validate_draft(&draft)?;
        let time = now();
        let scenario = Scenario {
            schema_version: 3,
            id: Uuid::new_v4().to_string(),
            name: draft.name.trim().to_string(),
            description: draft.description,
            tags: draft.tags,
            order: draft.order,
            strategies: draft.strategies,
            historical_selection: draft.historical_selection,
            latest_successful_run_id: None,
            last_attempt_run_id: None,
            created_at: time,
            updated_at: time,
        };
        self.save(&scenario)?;
        Ok(scenario)
    }

    pub fn save(&self, scenario: &Scenario) -> Result<(), String> {
        atomic_json(&self.scenario_path(&scenario.id), scenario)
    }

    pub fn update(&self, id: &str, draft: ScenarioDraft) -> Result<Scenario, String> {
        validate_draft(&draft)?;
        let mut scenario = self.get(id)?;
        scenario.schema_version = 3;
        scenario.name = draft.name.trim().into();
        scenario.description = draft.description;
        scenario.tags = draft.tags;
        scenario.order = draft.order;
        scenario.strategies = draft.strategies;
        scenario.historical_selection = draft.historical_selection;
        scenario.updated_at = now();
        self.save(&scenario)?;
        Ok(scenario)
    }

    pub fn duplicate(&self, id: &str) -> Result<Scenario, String> {
        let source = self.get(id)?;
        let names: Vec<_> = self.list().into_iter().map(|s| s.name).collect();
        let base = format!("{} Copy", source.name);
        let mut name = base.clone();
        let mut suffix = 2;
        while names.iter().any(|existing| existing == &name) {
            name = format!("{base} {suffix}");
            suffix += 1;
        }
        self.create(ScenarioDraft {
            name,
            description: source.description,
            tags: source.tags,
            order: source.order,
            strategies: source.strategies,
            historical_selection: source.historical_selection,
        })
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let path = self.scenario_path(id).parent().unwrap().to_path_buf();
        fs::remove_dir_all(&path).map_err(|e| format!("cannot delete {}: {e}", path.display()))
    }

    pub fn save_run(&self, run: &RunRecord) -> Result<(), String> {
        atomic_json(
            &self.run_dir(&run.scenario_id, &run.id).join("run.json"),
            run,
        )
    }

    pub fn get_run(&self, scenario: &str, run: &str) -> Result<RunRecord, String> {
        let path = self.run_dir(scenario, run).join("run.json");
        serde_json::from_str(
            &fs::read_to_string(&path).map_err(|e| format!("cannot read run: {e}"))?,
        )
        .map_err(|e| format!("cannot parse run: {e}"))
    }
}

fn validate_draft(draft: &ScenarioDraft) -> Result<(), String> {
    if draft.name.trim().is_empty() {
        return Err("scenario name is required".into());
    }
    if draft.order.symbol.trim().is_empty() {
        return Err("symbol is required".into());
    }
    match draft.order.target_mode {
        crate::model::TargetMode::Quantity
            if !draft.order.quantity.is_finite() || draft.order.quantity <= 0.0 =>
        {
            return Err("quantity must be positive".into());
        }
        crate::model::TargetMode::Notional
            if !draft
                .order
                .notional
                .is_some_and(|v| v.is_finite() && v > 0.0) =>
        {
            return Err("notional target must be positive".into());
        }
        _ => {}
    }
    if draft.strategies.len() != 1 || draft.strategies[0].name != "twap_sell" {
        return Err("prototype requires twap_sell".into());
    }
    if !matches!(
        draft.historical_selection.mode.as_str(),
        "all_available" | "latest_10"
    ) {
        return Err("historical selection must be all_available or latest_10".into());
    }
    let input = &draft.strategies[0].inputs;
    if input.start_times.is_empty()
        || input.time_taken_seconds.is_empty()
        || input.trade_frequency_seconds.is_empty()
    {
        return Err("strategy parameter lists cannot be empty".into());
    }
    for start_time in &input.start_times {
        parse_start_time(start_time)?;
    }
    if input
        .time_taken_seconds
        .iter()
        .chain(input.trade_frequency_seconds.iter())
        .any(|v| !v.is_finite() || *v <= 0.0)
    {
        return Err("strategy parameters must be positive finite numbers".into());
    }
    if input.start_times.len()
        * input.time_taken_seconds.len()
        * input.trade_frequency_seconds.len()
        > 500
    {
        return Err("maximum 500 configurations".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{HistoricalSelection, OrderSpec, Side, StrategySpec, TwapInputs};

    fn draft(name: &str) -> ScenarioDraft {
        ScenarioDraft {
            name: name.into(),
            description: String::new(),
            tags: vec![],
            order: OrderSpec {
                symbol: "VCB".into(),
                side: Side::Buy,
                target_mode: crate::model::TargetMode::Quantity,
                quantity: 1_000.0,
                notional: None,
            },
            strategies: vec![StrategySpec {
                name: "twap_sell".into(),
                inputs: TwapInputs {
                    start_times: vec!["10:00:00".into(), "10:30:00".into()],
                    time_taken_seconds: vec![1_800.0, 3_600.0],
                    trade_frequency_seconds: vec![30.0, 60.0],
                },
            }],
            historical_selection: HistoricalSelection::default(),
        }
    }

    #[test]
    fn rename_preserves_identity_and_result_pointer() {
        let root = std::env::temp_dir().join(format!("trader-viz-test-{}", Uuid::new_v4()));
        let store = Store::new(root.clone()).unwrap();
        let mut original = store.create(draft("Original")).unwrap();
        original.latest_successful_run_id = Some("successful-run".into());
        store.save(&original).unwrap();
        let renamed = store.update(&original.id, draft("Renamed")).unwrap();
        assert_eq!(renamed.id, original.id);
        assert_eq!(
            renamed.latest_successful_run_id.as_deref(),
            Some("successful-run")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_gets_new_identity_and_no_result_pointer() {
        let root = std::env::temp_dir().join(format!("trader-viz-test-{}", Uuid::new_v4()));
        let store = Store::new(root.clone()).unwrap();
        let mut original = store.create(draft("Original")).unwrap();
        original.latest_successful_run_id = Some("successful-run".into());
        store.save(&original).unwrap();
        let copy = store.duplicate(&original.id).unwrap();
        assert_ne!(copy.id, original.id);
        assert_eq!(copy.name, "Original Copy");
        assert!(copy.latest_successful_run_id.is_none());
        let _ = fs::remove_dir_all(root);
    }
}
