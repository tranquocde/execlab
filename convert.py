"""Convert VPS RTDS parquet sessions to HftBacktest NPZ files.

By default this scans ``data/1h/*.parquet`` and writes one file per symbol and
session to ``vps_data/1h/<SYMBOL>/<SESSION>.npz``. Existing session-derived
files are replaced atomically because a parquet session may receive more data.
Unrelated files, such as the legacy ``<SYMBOL>.npz``, are never removed.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path

import numpy as np
import pandas as pd

from hftbacktest.data.utils.vps import convert


DEFAULT_SYMBOLS = ["VNM", "MCH", "STB", "TCX", "VCK", "BID", "VCB"]
EXPECTED_DTYPE = (
    "ev",
    "exch_ts",
    "local_ts",
    "px",
    "qty",
    "order_id",
    "ival",
    "fval",
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input-dir", type=Path, default=Path("data/1h"))
    parser.add_argument("--output-dir", type=Path, default=Path("vps_data/1h"))
    parser.add_argument(
        "--symbols",
        nargs="+",
        default=DEFAULT_SYMBOLS,
        help="symbols to convert (default: %(default)s)",
    )
    return parser.parse_args()


def valid_rows(frame: pd.DataFrame, symbols: set[str]) -> tuple[pd.DataFrame, int]:
    """Drop target-symbol stock events whose side is neither B nor S."""
    keep: list[bool] = []
    invalid = 0
    for event, raw_data in frame[["event", "data"]].itertuples(index=False, name=None):
        valid = True
        if event == "stock":
            try:
                payload = json.loads(raw_data).get("data", {})
            except (TypeError, json.JSONDecodeError):
                payload = {}
            if payload.get("sym") in symbols and payload.get("side") not in ("B", "S"):
                valid = False
                invalid += 1
        keep.append(valid)
    return frame.loc[keep], invalid


def validate(path: Path, expected_count: int) -> None:
    with np.load(path, allow_pickle=False) as archive:
        if "data" not in archive:
            raise RuntimeError(f"{path} has no 'data' array")
        data = archive["data"]
        if data.dtype.names != EXPECTED_DTYPE:
            raise RuntimeError(f"{path} has unexpected dtype {data.dtype.names}")
        if len(data) != expected_count:
            raise RuntimeError(
                f"{path} contains {len(data)} events; expected {expected_count}"
            )


def event_count(path: Path) -> int | None:
    if not path.exists():
        return None
    with np.load(path, allow_pickle=False) as archive:
        return len(archive["data"])


def main() -> None:
    args = parse_args()
    sources = sorted(args.input_dir.glob("*.parquet"))
    if not sources:
        raise SystemExit(f"no parquet files found in {args.input_dir}")

    symbols = list(dict.fromkeys(symbol.upper() for symbol in args.symbols))
    symbol_set = set(symbols)
    converted = 0

    for source in sources:
        frame = pd.read_parquet(source, columns=["pit_timestamp", "event", "data"])
        filtered, invalid = valid_rows(frame, symbol_set)
        temporary_source = source.with_name(f".{source.name}.convert.tmp")
        filtered.to_parquet(temporary_source, index=False)
        try:
            print(f"SESSION {source.name}: excluded {invalid} invalid blank-side trades")
            for symbol in symbols:
                asset_dir = args.output_dir / symbol
                asset_dir.mkdir(parents=True, exist_ok=True)
                destination = asset_dir / f"{source.stem}.npz"
                temporary_output = asset_dir / f".{source.stem}.convert.tmp.npz"
                old_count = event_count(destination)
                temporary_output.unlink(missing_ok=True)
                try:
                    events = convert(
                        asset_name=symbol,
                        input_filename=str(temporary_source),
                        output_filename=str(temporary_output),
                        verbose=False,
                    )
                    validate(temporary_output, len(events))
                    os.replace(temporary_output, destination)
                finally:
                    temporary_output.unlink(missing_ok=True)
                action = "created" if old_count is None else "replaced"
                old = "-" if old_count is None else f"{old_count:,}"
                print(
                    f"  {action:8} {symbol:4} old={old:>8} "
                    f"new={len(events):>8,}  {destination}"
                )
                converted += 1
        finally:
            temporary_source.unlink(missing_ok=True)

    print(f"DONE: converted {converted} symbol/session files; unrelated files preserved")


if __name__ == "__main__":
    main()
