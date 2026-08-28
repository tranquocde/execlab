use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};

pub struct Config {
    pub workspace: PathBuf,
    pub data_dir: PathBuf,
    pub timeframe: String,
    pub state_dir: PathBuf,
    pub bind: String,
}

fn resolve(workspace: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    }
}

pub fn load() -> Result<Config, String> {
    let workspace = env::current_dir().map_err(|e| e.to_string())?;
    let mut values = HashMap::new();
    let env_path = workspace.join("trader_viz/.env");
    if let Ok(text) = fs::read_to_string(&env_path) {
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                values.insert(
                    key.trim().to_string(),
                    value.trim().trim_matches(['\'', '"']).to_string(),
                );
            }
        }
    }
    let get = |key: &str, default: &str| {
        env::var(key)
            .ok()
            .or_else(|| values.get(key).cloned())
            .unwrap_or_else(|| default.into())
    };
    Ok(Config {
        data_dir: resolve(&workspace, &get("TRADER_VIZ_DATA_DIR", "vps_data")),
        timeframe: get("TRADER_VIZ_TIMEFRAME", "1h"),
        state_dir: resolve(&workspace, &get("TRADER_VIZ_STATE_DIR", "trader_viz/state")),
        bind: get("TRADER_VIZ_BIND", "127.0.0.1:8790"),
        workspace,
    })
}
