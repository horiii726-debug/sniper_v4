import os
import sys
import tempfile

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from legacy_v4_guard import require_v4_snapshots  # noqa: E402


def _write(d, header):
    with open(os.path.join(d, "snapshots.csv"), "w") as f:
        f.write(header + "\n1,2,3\n")


def test_guard_rejects_v5_layout():
    with tempfile.TemporaryDirectory() as d:
        _write(d, "coin,order,time,is_signal,in_window,age_s,y_tp1,y_bin")
        with pytest.raises(SystemExit, match="V5 snapshot layout"):
            require_v4_snapshots(d)


def test_guard_accepts_v4_layout():
    with tempfile.TemporaryDirectory() as d:
        _write(d, "coin,order,time,age_s,y_up2,y_up5,y_cont,y_rug")
        assert require_v4_snapshots(d) == os.path.join(d, "snapshots.csv")


def test_guard_missing_file():
    with tempfile.TemporaryDirectory() as d:
        with pytest.raises(SystemExit, match="missing"):
            require_v4_snapshots(d)
