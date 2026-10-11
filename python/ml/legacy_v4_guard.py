"""V5-17: the LEGACY V4 training scripts read `<dir>/snapshots.csv` with V4 labels (y_up2, y_up5, y_cont, y_rug).
`sniper snapshots` now writes `snapshots_v5.csv.zst` (V5 labels, 10 h horizon); the V4 labels were right-censored
(V4-2), so no V4 writer is kept. This guard stops the V4 scripts with instructions instead of failing obscurely."""
import os
import sys

V4_LABELS = {"y_up2", "y_up5", "y_cont", "y_rug"}
V5_MARKERS = {"is_signal", "in_window", "y_bin"}
HOWTO = ("LEGACY V4 script. Use python/ml/v5_train.py on snapshots_v5.csv.zst, or run this script from a git "
         "worktree at commit e75a8c5 (V4) on V4 snapshot files.")


def require_v4_snapshots(d):
    """Path of a V4-layout snapshots.csv in directory d, else SystemExit with instructions."""
    p = os.path.join(d, "snapshots.csv")
    if not os.path.exists(p):
        raise SystemExit(f"{p}: missing. {HOWTO}")
    with open(p) as f:
        header = set(f.readline().strip().split(","))
    if header & V5_MARKERS:
        raise SystemExit(f"{p}: V5 snapshot layout. {HOWTO}")
    missing = V4_LABELS - header
    if missing:
        raise SystemExit(f"{p}: no V4 label columns {sorted(missing)}. {HOWTO}")
    return p


if __name__ == "__main__":
    print(require_v4_snapshots(sys.argv[1]))
