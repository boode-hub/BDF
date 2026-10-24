#!/usr/bin/env python3
"""Fails CI when a `bdf --bench` report exceeds the performance budget."""
import json
import sys

# Budgets for a 2000-page document on a CI runner (no GPU, software rendering).
LIMITS = {
    "open_ms": 4000,            # process start to document open
    "first_image_ms": 5000,     # process start to the first page image on screen
    "first_render_ms": 6000,    # process start to first page fully drawn
    "scroll_frame_ms_p95": 40,  # UI work per frame while scrolling
    "frame_ms_p95": 60,         # UI work per frame across all phases
    "peak_rss_mb": 600,         # peak memory, all included
    "idle_cpu_percent": 3.0,    # CPU when nothing happens
    "search_ms": 20000,         # search all 2000 pages
}

def main(path):
    with open(path) as f:
        r = json.load(f)
    print(json.dumps(r, indent=2))
    problems = list(r.get("failures") or [])
    for key, limit in LIMITS.items():
        v = r.get(key)
        if v is None:
            if key in ("peak_rss_mb",) and r.get("os") == "macos":
                continue
            problems.append(f"{key}: missing")
        elif v > limit:
            problems.append(f"{key}: {v:.1f} > {limit}")
    if r.get("search_hits", 0) < 1:
        problems.append("search found nothing")
    if r.get("scroll_frames", 0) < 250:
        problems.append("too few frames measured")
    if problems:
        print("\nBenchmark budget exceeded:\n  " + "\n  ".join(problems))
        sys.exit(1)
    print("\nBenchmark within budget.")

if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "bdf-bench.json")
