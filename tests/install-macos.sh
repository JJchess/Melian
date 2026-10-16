#!/bin/bash
# Destructive only to a disposable GitHub-hosted runner's Melian installation.
set -euo pipefail
[[ "${GITHUB_ACTIONS:-}" == true && "$(uname -s)" == Darwin ]] || { echo 'Run on a disposable macOS CI runner only.' >&2; exit 1; }
root="$(cd "$(dirname "$0")/.." && pwd)"
app="$HOME/Applications/Melian.app"
settings="$HOME/Library/Application Support/Melian/settings.json"
agent="$HOME/Library/LaunchAgents/io.github.jjchess.melian.plist"
[[ ! -e "$app" && ! -L "$app" && ! -e "$agent" ]] || { echo 'Refusing to replace an existing installation.' >&2; exit 1; }
case "$(uname -m)" in arm64) arch=arm64;; *) arch=x64;; esac
fixture="$(mktemp -d)"
trap 'rm -rf -- "$fixture"' EXIT
export MELIAN_TEST_ASSETS="$fixture/assets"
mkdir -p "$fixture/bin" "$MELIAN_TEST_ASSETS"
asset="Melian-macos-$arch.tar.gz"
cp "$root/rust/dist/$asset" "$MELIAN_TEST_ASSETS/$asset"
printf '{"tag_name":"v2.2.0","draft":false}\n' > "$MELIAN_TEST_ASSETS/release.json"
cat > "$fixture/bin/curl" <<'SH'
#!/bin/bash
set -eu
url=''; output=''
while [[ $# -gt 0 ]]; do
  case "$1" in https://*) url="$1"; shift;; -o) output="$2"; shift 2;; *) shift;; esac
done
case "$url" in
  https://api.github.com/repos/JJchess/Melian/releases/*) file=release.json;;
  https://github.com/JJchess/Melian/releases/download/v2.2.0/*) file="${url##*/}";;
  *) exit 90;;
esac
cp "$MELIAN_TEST_ASSETS/$file" "$output"
SH
chmod +x "$fixture/bin/curl"
export PATH="$fixture/bin:$PATH"
checksum() { (cd "$MELIAN_TEST_ASSETS" && shasum -a 256 "$asset" > SHA256SUMS.txt); }
expect_failure() {
  if bash "$root/install.sh" --no-start; then echo 'Expected installer failure.' >&2; exit 1; fi
}

# Corrupt downloads must fail before any installed files or hooks are changed.
printf '%064d  %s\n' 0 "$asset" > "$MELIAN_TEST_ASSETS/SHA256SUMS.txt"
expect_failure
[[ ! -e "$app" && ! -e "$agent" ]]
checksum
bash "$root/install.sh" --no-start
codesign --verify --deep --strict "$app"
[[ -L "$HOME/.local/bin/melian" && -L "$HOME/.local/bin/claude-guard" && -f "$agent" ]]
[[ "$(plutil -extract RunAtLoad raw -o - "$agent")" == true ]]

# Start the actual app, query IPC and prove the proxy defaults to denial with no
# controller present. No real Claude credentials or external requests are used.
"$app/Contents/MacOS/ClaudeGuard" > "$fixture/app.log" 2>&1 &
guard_pid=$!
for attempt in {1..40}; do
  if "$app/Contents/MacOS/claude-guard" --status > "$fixture/status.json" 2>/dev/null; then break; fi
  sleep 0.25
done
node - "$fixture/status.json" <<'JS'
const fs = require('node:fs'), assert = require('node:assert/strict');
const value = JSON.parse(fs.readFileSync(process.argv[2]));
assert.equal(value.ok, true); assert.equal(value.state.allowed, false);
JS
result="$(/usr/bin/curl --silent --max-time 3 --noproxy '' --proxy http://127.0.0.1:17890 --output /dev/null --write-out '%{http_code}' http://127.0.0.1:9/)"
[[ "$result" == 503 ]]
# Upgrade with the old instance still running, then require it to exit.
bash "$root/install.sh" --no-start
for attempt in {1..40}; do
  if ! kill -0 "$guard_pid" 2>/dev/null; then break; fi
  sleep 0.25
done
if kill -0 "$guard_pid" 2>/dev/null; then kill "$guard_pid"; echo 'App did not exit.' >&2; exit 1; fi
wait "$guard_pid"

# Upgrade must preserve a disabled auto-start preference and original CLI args.
node - "$settings" <<'JS'
const fs = require('node:fs'); const file = process.argv[2];
const value = JSON.parse(fs.readFileSync(file));
value.autoStart = false; value.preferredNode = 'saved node';
fs.writeFileSync(file, JSON.stringify(value));
JS
bash "$root/install.sh" --no-start
[[ ! -e "$agent" ]]
node - "$settings" <<'JS'
const fs = require('node:fs'), assert = require('node:assert/strict');
const value = JSON.parse(fs.readFileSync(process.argv[2]));
assert.equal(value.autoStart, false); assert.equal(value.preferredNode, 'saved node');
JS
"$HOME/.local/bin/claude-guard" --guard-help
cp "$settings" "$fixture/settings-before"
installed_hash="$(shasum -a 256 "$app/Contents/MacOS/ClaudeGuard")"

# A correctly signed package whose post-install step fails must roll back.
mkdir "$fixture/bad"
tar -xzf "$MELIAN_TEST_ASSETS/$asset" -C "$fixture/bad"
cp /usr/bin/false "$fixture/bad/Melian.app/Contents/MacOS/ClaudeGuard"
codesign --force --deep --sign - "$fixture/bad/Melian.app"
COPYFILE_DISABLE=1 tar -czf "$MELIAN_TEST_ASSETS/$asset" -C "$fixture/bad" Melian.app
checksum
expect_failure
[[ "$(shasum -a 256 "$app/Contents/MacOS/ClaudeGuard")" == "$installed_hash" ]]
cmp "$fixture/settings-before" "$settings"
[[ ! -e "$agent" && -L "$HOME/.local/bin/claude-guard" ]]

bash "$root/install.sh" --uninstall
[[ ! -e "$app" && ! -e "$agent" && ! -L "$HOME/.local/bin/melian" && ! -L "$HOME/.local/bin/claude-guard" ]]
[[ -f "$settings" ]]
for profile in "$HOME/.zprofile" "$HOME/.bash_profile"; do
  if [[ -f "$profile" ]] && grep -q '^# Melian CLI path$' "$profile"; then exit 1; fi
done
# First-install failure must remove its partial application and hooks, too.
expect_failure
[[ ! -e "$app" && ! -e "$agent" && ! -L "$HOME/.local/bin/melian" ]]
echo 'Installer: checksum, fresh install, running upgrade, preferences, rollback and uninstall passed.'
