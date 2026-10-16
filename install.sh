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
if $uninstall; then
  owns_app "$app" || fail '未找到本安装器管理的 Melian.app。'
  "$app/Contents/MacOS/claude-guard" --quit >/dev/null 2>&1 || true
  "$app/Contents/MacOS/ClaudeGuard" --uninstall-hooks
  for tool in melian claude-guard; do
    link="$HOME/.local/bin/$tool"
    target="$app/Contents/MacOS/ClaudeGuard"; [[ "$tool" == claude-guard ]] && target="$app/Contents/MacOS/claude-guard"
    if [[ -L "$link" && "$(readlink "$link")" == "$target" ]]; then rm -- "$link"; fi
  done
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
committed=false
cleanup() {
  code=$?
  if [[ -n "$backup" && -d "$backup" ]] && ! $committed; then
    if owns_app "$app"; then rm -rf -- "$app"; fi
    [[ ! -e "$app" ]] && mv -- "$backup" "$app"
    "$app/Contents/MacOS/ClaudeGuard" --install-hooks >/dev/null 2>&1 || true
  fi
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
for tool in melian claude-guard; do
  link="$HOME/.local/bin/$tool"
  target="$app/Contents/MacOS/ClaudeGuard"; [[ "$tool" == claude-guard ]] && target="$app/Contents/MacOS/claude-guard"
  if [[ -e "$link" || -L "$link" ]]; then [[ -L "$link" && "$(readlink "$link")" == "$target" ]] || fail "命令路径已被其他程序占用：$link"; fi
done
if [[ -e "$app" || -L "$app" ]]; then
  owns_app "$app" || fail '目标应用不属于 Melian，未替换。'
  "$app/Contents/MacOS/claude-guard" --quit >/dev/null 2>&1 || true
  backup="$HOME/Applications/.Melian-rollback-$$.app"
  [[ ! -e "$backup" ]] || fail '回退目录已存在。'
  mv -- "$app" "$backup"
fi
mv -- "$new_app" "$app"
"$app/Contents/MacOS/ClaudeGuard" --install-hooks
ln -sfn -- "$app/Contents/MacOS/ClaudeGuard" "$HOME/.local/bin/melian"
ln -sfn -- "$app/Contents/MacOS/claude-guard" "$HOME/.local/bin/claude-guard"
profile="$HOME/.zprofile"
[[ "${SHELL:-/bin/zsh}" == */bash ]] && profile="$HOME/.bash_profile"
if ! grep -q '^# Melian CLI path$' "$profile" 2>/dev/null; then
  printf '\n# Melian CLI path\nexport PATH="$HOME/.local/bin:$PATH"\n# End Melian CLI path\n' >> "$profile"
fi
committed=true
if [[ -n "$backup" ]] && owns_app "$backup"; then rm -rf -- "$backup"; fi
if ! $no_start; then open "$app"; fi
printf 'Melian %s 安装完成。可从 Applications 打开；新终端中可用 claude-guard。\n' "$tag"
