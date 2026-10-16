#!/bin/bash
# macOS: curl -fsSL https://raw.githubusercontent.com/JJchess/Melian/main/install.sh | bash
set -euo pipefail
fail() { printf 'Melian: %s\n' "$*" >&2; exit 1; }
[[ "$(uname -s)" == Darwin ]] || fail '此脚本用于 macOS；Windows 请使用 install.ps1。'
version=latest
download_only=false
uninstall=false
no_start=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) [[ $# -ge 2 ]] || fail '--version 缺少版本号'; version="$2"; shift 2;;
    --download-only) download_only=true; shift;;
    --no-start) no_start=true; shift;;
    --uninstall) uninstall=true; shift;;
    *) fail "未知参数：$1";;
  esac
done
app="$HOME/Applications/Melian.app"
bundle_id=io.github.jjchess.melian
owns_app() { [[ -d "$1" && ! -L "$1" ]] && [[ "$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$1/Contents/Info.plist" 2>/dev/null)" == "$bundle_id" ]]; }
quit_guard() {
  if "$app/Contents/MacOS/claude-guard" --quit >/dev/null 2>&1; then
    # An acknowledged command is queued on AppKit, not proof the old instance
    # released its socket. Wait before replacing or reopening the application.
    for attempt in {1..40}; do
      if ! "$app/Contents/MacOS/ClaudeGuard" --health-check >/dev/null 2>&1; then return 0; fi
      sleep 0.1
    done
    fail 'Melian 尚未退出，未替换应用。请从菜单退出后重试。'
  fi
}
remove_cli_links() {
  for tool in melian claude-guard; do
    link="$HOME/.local/bin/$tool"
    target="$app/Contents/MacOS/ClaudeGuard"; [[ "$tool" == claude-guard ]] && target="$app/Contents/MacOS/claude-guard"
    if [[ -L "$link" && "$(readlink "$link")" == "$target" ]]; then rm -- "$link"; fi
  done
}
remove_profile_block() {
  [[ -f "$1" ]] || return 0
  profile_temp="$(mktemp "$1.melian-cleanup.XXXXXXXX")"
  # Only remove the exact block inserted below; preserve edited user content.
  /usr/bin/awk '
    { lines[NR]=$0 }
    END { for(i=1;i<=NR;i++) {
      if(lines[i]=="# Melian CLI path" && lines[i+1]=="export PATH=\"$HOME/.local/bin:$PATH\"" && lines[i+2]=="# End Melian CLI path") {i+=2; continue}
      print lines[i]
    }}' "$1" > "$profile_temp"
  cat "$profile_temp" > "$1"
  rm -- "$profile_temp"
}
if $uninstall; then
  owns_app "$app" || fail '未找到本安装器管理的 Melian.app。'
  quit_guard
  "$app/Contents/MacOS/ClaudeGuard" --uninstall-hooks
  remove_cli_links
  remove_profile_block "$HOME/.zprofile"
  remove_profile_block "$HOME/.bash_profile"
  # Fixed, owned user application path; never operate on an arbitrary argument.
  rm -rf -- "$app"
  printf 'Melian 已卸载，设置和日志保留。\n'
  exit 0
fi
[[ "$version" == latest || "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]] || fail '无效的版本号。'
case "$(uname -m)" in arm64) arch=arm64;; x86_64) arch=x64;; *) fail '不支持的 Mac 架构。';; esac
[[ "$(sw_vers -productVersion | cut -d. -f1)" -ge 13 ]] || fail '需要 macOS 13 或更新版本。'
tmp="$(mktemp -d "${TMPDIR:-/tmp}/melian-install.XXXXXXXX")"
backup=''
staged=''
touched=false
committed=false
cleanup() {
  code=$?
  set +e
  if $touched && ! $committed; then
    if owns_app "$app"; then rm -rf -- "$app"; fi
    if [[ -n "$backup" && -d "$backup" && ! -e "$app" && ! -L "$app" ]]; then mv -- "$backup" "$app"; fi
    remove_cli_links
    for tool in melian claude-guard; do
      [[ ! -L "$tmp/$tool" ]] || cp -P "$tmp/$tool" "$HOME/.local/bin/$tool"
    done
    for file in settings agent profile; do
      case "$file" in settings) destination="$settings";; agent) destination="$agent";; profile) destination="$profile";; esac
      if [[ -f "$tmp/$file" ]]; then cp -p "$tmp/$file" "$destination"; else rm -f -- "$destination"; fi
    done
    if [[ -n "$backup" ]] && owns_app "$app" && ! $no_start; then open "$app"; fi
  fi
  if [[ -n "$staged" && -d "$staged" ]]; then rm -rf -- "$staged"; fi
  rm -rf -- "$tmp"
  return "$code"
}
trap cleanup EXIT
fetch() { curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 --retry 2 "$1" -o "$2"; }
api="https://api.github.com/repos/JJchess/Melian/releases/latest"
[[ "$version" == latest ]] || api="https://api.github.com/repos/JJchess/Melian/releases/tags/$version"
printf '正在获取 Melian 安装包…\n'
fetch "$api" "$tmp/release.json"
tag="$(/usr/bin/plutil -extract tag_name raw -o - "$tmp/release.json")"
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]] || fail '发布信息无效。'
[[ "$version" == latest || "$version" == "$tag" ]] || fail '发布版本不匹配。'
[[ "$(/usr/bin/plutil -extract draft raw -o - "$tmp/release.json")" == false ]] || fail '不能安装未发布版本。'
base="https://github.com/JJchess/Melian/releases/download/$tag"
asset="Melian-macos-$arch.tar.gz"
fetch "$base/SHA256SUMS.txt" "$tmp/SHA256SUMS.txt"
fetch "$base/$asset" "$tmp/$asset"
expected="$(awk -v name="$asset" '$2 == name && length($1) == 64 {print $1}' "$tmp/SHA256SUMS.txt")"
[[ "$expected" =~ ^[a-fA-F0-9]{64}$ ]] || fail '缺少唯一的安装包校验值。'
actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
[[ "$actual" == "$expected" ]] || fail '安装包校验失败。'
if $download_only; then cp -- "$tmp/$asset" "./$asset"; printf '校验通过：%s/%s\n' "$PWD" "$asset"; exit 0; fi
tar -tzf "$tmp/$asset" > "$tmp/entries"
while IFS= read -r entry; do
  case "$entry" in Melian.app|Melian.app/|Melian.app/*) ;; *) fail '安装包包含未知路径。';; esac
  case "/$entry/" in */../*|*/./*) fail '安装包路径无效。';; esac
done < "$tmp/entries"
if tar -tvzf "$tmp/$asset" | awk 'substr($1,1,1) != "-" && substr($1,1,1) != "d" {bad=1} END {exit !bad}'; then fail '安装包不允许符号链接或特殊文件。'; fi
mkdir "$tmp/stage"
tar -xzf "$tmp/$asset" -C "$tmp/stage"
new_app="$tmp/stage/Melian.app"
owns_app "$new_app" || fail '应用标识不匹配。'
codesign --verify --deep --strict "$new_app"
[[ -x "$new_app/Contents/MacOS/ClaudeGuard" && -x "$new_app/Contents/MacOS/claude-guard" ]] || fail '安装包不完整。'
mkdir -p "$HOME/Applications" "$HOME/.local/bin"
# Stage on the destination volume so replacing the app is a rename.
staged="$(mktemp -d "$HOME/Applications/.Melian-stage.XXXXXXXX")"
mv -- "$new_app" "$staged/Melian.app"
new_app="$staged/Melian.app"
settings="$HOME/Library/Application Support/Melian/settings.json"
agent="$HOME/Library/LaunchAgents/$bundle_id.plist"
profile="$HOME/.zprofile"
[[ "${SHELL:-/bin/zsh}" == */bash ]] && profile="$HOME/.bash_profile"
for file in settings agent profile; do
  case "$file" in settings) source="$settings";; agent) source="$agent";; profile) source="$profile";; esac
  if [[ -e "$source" ]]; then [[ -f "$source" ]] || fail "不是普通配置文件：$source"; cp -p "$source" "$tmp/$file"; fi
done
for tool in melian claude-guard; do
  link="$HOME/.local/bin/$tool"
  target="$app/Contents/MacOS/ClaudeGuard"; [[ "$tool" == claude-guard ]] && target="$app/Contents/MacOS/claude-guard"
  if [[ -e "$link" || -L "$link" ]]; then [[ -L "$link" && "$(readlink "$link")" == "$target" ]] || fail "命令路径已被其他程序占用：$link"; fi
  [[ ! -L "$link" ]] || cp -P "$link" "$tmp/$tool"
done
if [[ -e "$app" || -L "$app" ]]; then
  owns_app "$app" || fail '目标应用不属于 Melian，未替换。'
  quit_guard
  backup="$HOME/Applications/.Melian-rollback-$$.app"
  [[ ! -e "$backup" && ! -L "$backup" ]] || fail '回退目录已存在。'
  touched=true
  mv -- "$app" "$backup"
fi
touched=true
mv -- "$new_app" "$app"
"$app/Contents/MacOS/ClaudeGuard" --install-hooks
ln -sfn -- "$app/Contents/MacOS/ClaudeGuard" "$HOME/.local/bin/melian"
ln -sfn -- "$app/Contents/MacOS/claude-guard" "$HOME/.local/bin/claude-guard"
if ! grep -q '^# Melian CLI path$' "$profile" 2>/dev/null; then
  printf '\n# Melian CLI path\nexport PATH="$HOME/.local/bin:$PATH"\n# End Melian CLI path\n' >> "$profile"
fi
committed=true
if [[ -n "$backup" ]] && owns_app "$backup"; then rm -rf -- "$backup"; fi
if ! $no_start; then open "$app"; fi
printf 'Melian %s 安装完成。可从 Applications 打开；新终端中可用 claude-guard。\n' "$tag"
