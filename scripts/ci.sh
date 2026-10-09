#!/usr/bin/env bash
# Local CI (V4 §30, §31.1 rule 4): build, tests, secret hygiene. Exit != 0 on any failure.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

cargo build --release --quiet
cargo test --release --quiet 2>&1 | grep -E "^test result" | tee /dev/stderr | grep -qv "FAILED"
if cargo test --release --quiet 2>&1 | grep -q FAILED; then echo "CI FAIL: tests"; exit 1; fi

fail=0
# 1) .env / keypairs never tracked.
if git ls-files | grep -E '(^|/)\.env$|keypair|id\.json' ; then echo "CI FAIL: secret file tracked"; fail=1; fi
# 2) Secret values never in logs, derived output or tracked files.
key=""
[ -f .env ] && key=$(grep -E '^HELIUS_API_KEY=' .env | cut -d= -f2- || true)
scan_paths=(logs data/*/run.log data/*/derived*.txt data/*/summary.json)
if [ -n "$key" ]; then
  if grep -rIlF -- "$key" "${scan_paths[@]}" 2>/dev/null; then echo "CI FAIL: API key found in logs/output"; fail=1; fi
  if git grep -lF -- "$key" 2>/dev/null; then echo "CI FAIL: API key in tracked files"; fail=1; fi
fi
# 3) Any api-key query value that is not redacted.
if grep -rIhoE 'api-key=[^<&" ]+' "${scan_paths[@]}" 2>/dev/null | grep -v '<redacted>' | head -1 | grep -q .; then
  echo "CI FAIL: unredacted api-key= in logs"; fail=1
fi
# 4) Redaction smoke: log a line containing the key through the real logger, then grep it.
if [ -n "$key" ]; then
  tmp=$(mktemp -d)
  ./target/release/sniper record --cluster devnet --feeds grpc --profile venues --out "$tmp/rec" --duration-secs 3 >/dev/null 2>"$tmp/stderr" || true
  if grep -qF -- "$key" "$tmp/stderr" "$tmp/rec/run.log" 2>/dev/null; then echo "CI FAIL: key leaked in smoke run"; fail=1; fi
  rm -rf "$tmp"
fi
[ $fail -eq 0 ] && echo "CI PASS"
exit $fail
