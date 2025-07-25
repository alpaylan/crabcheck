import json
from pathlib import Path
from collections import defaultdict
from typing import Dict, Tuple, List
from math import log2

RegionKey = Tuple[str, int, int, int, int]

def extract_region_counts(json_path: Path) -> Dict[RegionKey, int]:
    with open(json_path) as f:
        data = json.load(f)

    regions = defaultdict(int)
    entry = data["data"][0]

    for func in entry["functions"]:
        for region in func.get("regions", []):
            sl, sc, el, ec, count = region[:5]
            
            fname = func["filenames"][0]
            key = (fname, func["name"], sl, sc, el, ec)
            regions[key] += count
    return regions

def aggregate(paths: List[Path]) -> Dict[RegionKey, int]:
    total = defaultdict(int)
    for path in paths:
        counts = extract_region_counts(path)
        for region, count in counts.items():
            total[region] += count
    return total

# read jsondata/indices.json
indices = Path("target/llvm-cov-target/indices.json")
with open(indices) as f:
    data = json.load(f)
    positive_indices = data["positives"]
    negative_indices = data["negatives"]

def snapshot_path(i): return Path(f"jsondata/demangled/snapshot_iteration_{i}.json")

positive_paths = [snapshot_path(i) for i in positive_indices]
negative_paths = [snapshot_path(i) for i in negative_indices]

# Aggregate
pos_cov = aggregate(positive_paths)
neg_cov = aggregate(negative_paths)

# Compare
all_regions = sorted(set(pos_cov) | set(neg_cov))

print(f"{'File:Line':60} {'Pos':>8} {'Neg':>8} {'Δ':>8}")
print("-" * 84)
for region in all_regions:
    pos = round(pos_cov.get(region, 0) / len(positive_paths), 2)
    neg = round(neg_cov.get(region, 0) / len(negative_paths), 2)
    delta = round(neg - pos, 2)
    fname, func, sl, sc, el, ec = region
    label = f"({func}){Path(fname).name}:{sl}:{sc} -> {el}:{ec}"
    if delta > 0:
        print(f"{label:60} {pos:8} {neg:8} {delta:+8}")
