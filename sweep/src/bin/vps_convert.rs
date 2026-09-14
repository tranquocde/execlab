use std::{
    collections::{HashMap, HashSet},
    env,
    fs::{self, File},
    path::{Path, PathBuf},
};

use hftbacktest::{
    backtest::data::{read_npz_file, write_npy},
    types::{Event, BUY_EVENT, DEPTH_EVENT, EXCH_EVENT, LOCAL_EVENT, SELL_EVENT, TRADE_EVENT},
};
use parquet::{
    file::reader::{FileReader, SerializedFileReader},
    record::Field,
};
use serde_json::Value;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const DEFAULT_SYMBOLS: &[&str] = &["VNM", "MCH", "STB", "TCX", "VCK", "BID", "VCB"];
const DAY_NS: i64 = 86_400_000_000_000;
const ICT_OFFSET_NS: i64 = 7 * 3_600_000_000_000;

#[derive(Debug)]
struct Args {
    input_dir: PathBuf,
    output_dir: PathBuf,
    symbols: Vec<String>,
}

fn usage() -> &'static str {
    "usage: vps_convert [--input-dir data/1h] [--output-dir vps_data/1h] \\\n     [--symbols VNM MCH STB TCX VCK BID VCB]"
}

fn parse_args() -> Result<Args, String> {
    let mut input_dir = PathBuf::from("data/1h");
    let mut output_dir = PathBuf::from("vps_data/1h");
    let mut symbols = Vec::new();
    let values: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;
    while i < values.len() {
        match values[i].as_str() {
            "--input-dir" => {
                i += 1;
                input_dir = values.get(i).ok_or("--input-dir needs a path")?.into();
            }
            "--output-dir" => {
                i += 1;
                output_dir = values.get(i).ok_or("--output-dir needs a path")?.into();
            }
            "--symbols" => {
                i += 1;
                while i < values.len() && !values[i].starts_with("--") {
                    symbols.push(values[i].to_uppercase());
                    i += 1;
                }
                if symbols.is_empty() {
                    return Err("--symbols needs at least one symbol".into());
                }
                continue;
            }
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}\n{}", usage())),
        }
        i += 1;
    }
    if symbols.is_empty() {
        symbols = DEFAULT_SYMBOLS.iter().map(|v| (*v).into()).collect();
    }
    let mut seen = HashSet::new();
    symbols.retain(|symbol| seen.insert(symbol.clone()));
    Ok(Args {
        input_dir,
        output_dir,
        symbols,
    })
}

fn parquet_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path.extension().is_some_and(|ext| ext == "parquet")
                && !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(format!("no parquet files found in {}", dir.display()));
    }
    Ok(files)
}

fn field<'a>(row: &'a parquet::record::Row, name: &str) -> Result<&'a Field, String> {
    row.get_column_iter()
        .find_map(|(column, value)| (column == name).then_some(value))
        .ok_or_else(|| format!("missing Parquet column {name}"))
}

fn timestamp_ns(value: &Field) -> Result<i64, String> {
    match value {
        Field::Long(value) => Ok(*value),
        Field::TimestampMicros(value) => value
            .checked_mul(1_000)
            .ok_or_else(|| "timestamp overflows nanoseconds".into()),
        Field::TimestampMillis(value) => value
            .checked_mul(1_000_000)
            .ok_or_else(|| "timestamp overflows nanoseconds".into()),
        other => Err(format!("unsupported pit_timestamp field: {other:?}")),
    }
}

fn text(value: &Field, column: &str) -> Result<String, String> {
    match value {
        Field::Str(value) => Ok(value.clone()),
        Field::Bytes(value) => String::from_utf8(value.data().to_vec())
            .map_err(|e| format!("invalid UTF-8 in {column}: {e}")),
        other => Err(format!("unsupported {column} field: {other:?}")),
    }
}

fn json_str<'a>(data: &'a Value, key: &str) -> Result<&'a str, String> {
    data.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string field {key}"))
}

fn json_f64(data: &Value, key: &str) -> Result<f64, String> {
    let value = data
        .get(key)
        .ok_or_else(|| format!("missing numeric field {key}"))?;
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
        .ok_or_else(|| format!("invalid numeric field {key}: {value}"))
}

fn parse_level(value: &str) -> Result<(f64, f64), String> {
    let mut fields = value.split('|');
    let px = fields
        .next()
        .ok_or("book level has no price")?
        .parse::<f64>()
        .map_err(|e| format!("invalid book price: {e}"))?;
    let qty = fields
        .next()
        .ok_or("book level has no quantity")?
        .parse::<f64>()
        .map_err(|e| format!("invalid book quantity: {e}"))?;
    Ok((px * 1_000.0, qty))
}

fn exchange_timestamp(local_ts: i64, time: Option<&str>) -> Result<i64, String> {
    let Some(time) = time.filter(|v| !v.is_empty()) else {
        return Ok(local_ts);
    };
    let mut parts = time.split(':');
    let hour: i64 = parts
        .next()
        .ok_or("missing hour")?
        .parse()
        .map_err(|e| format!("invalid hour in {time}: {e}"))?;
    let minute: i64 = parts
        .next()
        .ok_or("missing minute")?
        .parse()
        .map_err(|e| format!("invalid minute in {time}: {e}"))?;
    let second: i64 = parts
        .next()
        .ok_or("missing second")?
        .parse()
        .map_err(|e| format!("invalid second in {time}: {e}"))?;
    if parts.next().is_some() || hour >= 24 || minute >= 60 || second >= 60 {
        return Err(format!("invalid time {time}"));
    }
    let ict_day = (local_ts + ICT_OFFSET_NS).div_euclid(DAY_NS);
    Ok(ict_day * DAY_NS - ICT_OFFSET_NS + (hour * 3_600 + minute * 60 + second) * 1_000_000_000)
}

fn event(ev: u64, exch_ts: i64, local_ts: i64, px: f64, qty: f64, ival: i64) -> Event {
    Event {
        ev,
        exch_ts,
        local_ts,
        px,
        qty,
        order_id: 0,
        ival,
        fval: 0.0,
    }
}

fn correct_local_timestamp(events: &mut [Event]) {
    let Some(min_latency) = events.iter().map(|v| v.local_ts - v.exch_ts).min() else {
        return;
    };
    if min_latency < 0 {
        let offset = -min_latency;
        println!("  local_timestamp is ahead of exch_timestamp by {offset}");
        for event in events {
            event.local_ts += offset;
        }
    }
}

fn correct_event_order(events: &[Event]) -> Vec<Event> {
    let mut exchange: Vec<usize> = (0..events.len()).collect();
    let mut local: Vec<usize> = (0..events.len()).collect();
    exchange.sort_by_key(|&i| events[i].exch_ts);
    local.sort_by_key(|&i| events[i].local_ts);
    let mut out = Vec::with_capacity(events.len() * 2);
    let (mut ei, mut li) = (0, 0);
    while ei < exchange.len() || li < local.len() {
        if ei == exchange.len() {
            let mut value = events[local[li]].clone();
            value.ev |= LOCAL_EVENT;
            out.push(value);
            li += 1;
            continue;
        }
        if li == local.len() {
            let mut value = events[exchange[ei]].clone();
            value.ev |= EXCH_EVENT;
            out.push(value);
            ei += 1;
            continue;
        }
        let exch_index = exchange[ei];
        let local_index = local[li];
        let exch = &events[exch_index];
        let local_event = &events[local_index];
        if exch_index == local_index {
            let mut value = exch.clone();
            value.ev |= EXCH_EVENT | LOCAL_EVENT;
            out.push(value);
            ei += 1;
            li += 1;
        } else if exch.exch_ts < local_event.exch_ts
            || (exch.exch_ts == local_event.exch_ts && exch.local_ts < local_event.local_ts)
        {
            let mut value = exch.clone();
            value.ev |= EXCH_EVENT;
            out.push(value);
            ei += 1;
        } else {
            let mut value = local_event.clone();
            value.ev |= LOCAL_EVENT;
            out.push(value);
            li += 1;
        }
    }
    out
}

fn validate_event_order(events: &[Event]) -> Result<(), String> {
    let mut exchange_ts = i64::MIN;
    let mut local_ts = i64::MIN;
    for event in events {
        if event.ev & EXCH_EVENT == EXCH_EVENT {
            if event.exch_ts < exchange_ts {
                return Err("exchange events are out of order".into());
            }
            exchange_ts = event.exch_ts;
        }
        if event.ev & LOCAL_EVENT == LOCAL_EVENT {
            if event.local_ts < local_ts {
                return Err("local events are out of order".into());
            }
            local_ts = event.local_ts;
        }
    }
    Ok(())
}

fn convert_session(
    path: &Path,
    symbols: &[String],
) -> Result<(HashMap<String, Vec<Event>>, usize), String> {
    let wanted: HashSet<_> = symbols.iter().map(String::as_str).collect();
    let mut events: HashMap<String, Vec<Event>> = symbols
        .iter()
        .cloned()
        .map(|symbol| (symbol, Vec::new()))
        .collect();
    let reader = SerializedFileReader::new(
        File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?,
    )
    .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let rows = reader
        .get_row_iter(None)
        .map_err(|e| format!("cannot iterate {}: {e}", path.display()))?;
    let mut invalid = 0;
    for (row_no, row) in rows.enumerate() {
        let row = row.map_err(|e| format!("{} row {}: {e}", path.display(), row_no + 1))?;
        let local_ts = timestamp_ns(field(&row, "pit_timestamp")?)?;
        let kind = text(field(&row, "event")?, "event")?;
        if kind != "board" && kind != "stock" {
            continue;
        }
        let raw = text(field(&row, "data")?, "data")?;
        let message: Value = serde_json::from_str(&raw).map_err(|e| {
            format!(
                "{} row {} has invalid JSON: {e}",
                path.display(),
                row_no + 1
            )
        })?;
        let Some(data) = message.get("data") else {
            continue;
        };
        let Some(symbol) = data.get("sym").and_then(Value::as_str) else {
            continue;
        };
        if !wanted.contains(symbol) {
            continue;
        }
        let side = data.get("side").and_then(Value::as_str);
        if kind == "stock" && !matches!(side, Some("B" | "S")) {
            invalid += 1;
            continue;
        }
        let id = data.get("id").and_then(Value::as_i64);
        if kind == "board" && id == Some(3210) {
            let side_flag = match side {
                Some("B") => BUY_EVENT,
                Some("S") => SELL_EVENT,
                other => return Err(format!("unsupported VPS board side: {other:?}")),
            };
            let exch_ts =
                exchange_timestamp(local_ts, data.get("timeServer").and_then(Value::as_str))?;
            for level in ["g1", "g2", "g3"] {
                let (px, qty) = parse_level(json_str(data, level)?)?;
                events.get_mut(symbol).unwrap().push(event(
                    DEPTH_EVENT | side_flag,
                    exch_ts,
                    local_ts,
                    px,
                    qty,
                    0,
                ));
            }
        } else if kind == "stock" && id == Some(3220) {
            let side_flag = if side == Some("B") {
                BUY_EVENT
            } else {
                SELL_EVENT
            };
            let time = data
                .get("time")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .or_else(|| data.get("timeServer").and_then(Value::as_str));
            let exch_ts = exchange_timestamp(local_ts, time)?;
            let total_volume = data
                .get("totalVol")
                .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
                .unwrap_or(0);
            events.get_mut(symbol).unwrap().push(event(
                TRADE_EVENT | side_flag,
                exch_ts,
                local_ts,
                json_f64(data, "lastPrice")?,
                json_f64(data, "lastVol")?,
                total_volume,
            ));
        }
    }
    for (symbol, values) in &mut events {
        if values.is_empty() {
            return Err(format!(
                "no VPS board or stock events found for asset_name={symbol:?} in {}",
                path.display()
            ));
        }
        correct_local_timestamp(values);
        *values = correct_event_order(values);
        validate_event_order(values)?;
    }
    Ok((events, invalid))
}

fn event_count(path: &Path) -> Result<Option<usize>, String> {
    if !path.exists() {
        return Ok(None);
    }
    read_npz_file::<Event>(path.to_str().ok_or("output path is not UTF-8")?, "data")
        .map(|data| Some(data.len()))
        .map_err(|e| format!("cannot read existing {}: {e}", path.display()))
}

fn write_npz(path: &Path, events: &[Event]) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9));
    archive
        .start_file("data.npy", options)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    write_npy(&mut archive, events).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    archive
        .finish()
        .map_err(|e| format!("cannot finish {}: {e}", path.display()))?;
    Ok(())
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let sources = parquet_files(&args.input_dir)?;
    let mut converted = 0;
    for source in sources {
        let (mut by_symbol, invalid) = convert_session(&source, &args.symbols)?;
        println!(
            "SESSION {}: excluded {} invalid blank-side trades",
            source.file_name().unwrap().to_string_lossy(),
            invalid
        );
        let stem = source.file_stem().unwrap().to_string_lossy();
        for symbol in &args.symbols {
            let events = by_symbol.remove(symbol).unwrap();
            let asset_dir = args.output_dir.join(symbol);
            fs::create_dir_all(&asset_dir)
                .map_err(|e| format!("cannot create {}: {e}", asset_dir.display()))?;
            let destination = asset_dir.join(format!("{stem}.npz"));
            let temporary = asset_dir.join(format!(".{stem}.convert.tmp.npz"));
            let old = event_count(&destination)?;
            if temporary.exists() {
                fs::remove_file(&temporary)
                    .map_err(|e| format!("cannot remove {}: {e}", temporary.display()))?;
            }
            let result = (|| {
                write_npz(&temporary, &events)?;
                let written = event_count(&temporary)?;
                if written != Some(events.len()) {
                    return Err(format!(
                        "{} contains {:?} events; expected {}",
                        temporary.display(),
                        written,
                        events.len()
                    ));
                }
                fs::rename(&temporary, &destination).map_err(|e| {
                    format!(
                        "cannot replace {} with {}: {e}",
                        destination.display(),
                        temporary.display()
                    )
                })
            })();
            if result.is_err() && temporary.exists() {
                let _ = fs::remove_file(&temporary);
            }
            result?;
            println!(
                "  {:8} {:4} old={:>8} new={:>8}  {}",
                if old.is_some() { "replaced" } else { "created" },
                symbol,
                old.map_or_else(|| "-".into(), |v| v.to_string()),
                events.len(),
                destination.display()
            );
            converted += 1;
        }
    }
    println!("DONE: converted {converted} symbol/session files; unrelated files preserved");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ict_date_and_time_are_combined() {
        let local = 1_786_517_002_559_834_000_i64; // 2026-08-12 07:00:02 UTC
        assert_eq!(
            exchange_timestamp(local, Some("14:00:01")).unwrap(),
            1_786_518_001_000_000_000
        );
    }

    #[test]
    fn duplicate_symbols_are_removed_without_reordering() {
        let mut symbols = vec!["VCB".to_string(), "BID".to_string(), "VCB".to_string()];
        let mut seen = HashSet::new();
        symbols.retain(|symbol| seen.insert(symbol.clone()));
        assert_eq!(symbols, ["VCB", "BID"]);
    }
}
