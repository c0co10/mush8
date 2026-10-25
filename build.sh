#!/bin/bash
# Mush8 - build.sh
# Constantin Clerc (c22dev) - 0.1
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]:-$0}")"

TOOLCHAIN=1.98.1
OUT="$HOME/Downloads/Mush8Output"
SAVED=build/.choices

bold() { printf '\n\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\n\033[31m%s\033[0m\n' "$*" >&2; exit 1; }
ask() {
  local answer
  read -r -e -p "$1 [$2]: " answer || exit 1
  answer="$(printf '%s' "$answer" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
  REPLY="${answer:-$2}"
}
pick() {
  local prompt="$1"; shift
  bold "$prompt"
  PS3="Choose 1-$#: "
  select choice in "$@"; do
    [ -n "${choice:-}" ] && return
    echo "Type a number between 1 and $#."
  done
  exit 1
}
clean_path() {
  local p="$1"
  p="${p#\'}"; p="${p%\'}"; p="${p#\"}"; p="${p%\"}"
  p="${p//\\ / }"
  p="${p/#\~/$HOME}"
  printf '%s' "$p"
}
xcode_teams() {
  defaults export com.apple.dt.Xcode - 2>/dev/null | /usr/bin/python3 -I -c '
import plistlib, sys
try: d = plistlib.loads(sys.stdin.buffer.read())
except Exception: sys.exit()
seen = set()
for teams in d.get("IDEProvisioningTeamByIdentifier", {}).values():
    for t in teams:
        if t.get("teamID") and t["teamID"] not in seen:
            seen.add(t["teamID"])
            print("%s\t%d\t%s" % (t["teamID"], bool(t.get("isFreeProvisioningTeam")), t.get("teamName", t["teamID"])))
' || true
}
run() {
  mkdir -p build
  if "$@" 2>&1 | tee build/build.log; then return; fi
  explain
}
explain() {
  local log=build/build.log hint=""
  if grep -E "No Account for Team|No signing certificate|No profiles for|requires a provisioning profile" "$log" >/dev/null; then
    hint="Xcode couldn't sign the app. Open Xcode > Settings > Accounts, check you're signed in with your Apple ID, then run this again."
  elif grep -E "extended-virtual-addressing|increased-memory-limit|multicast" "$log" >/dev/null; then
    hint="Your Apple account can't use an entitlement the game needs. Run this again and choose \"Make an .ipa\" (AltStore / SideStore), which works with a free Apple ID."
  elif grep -E "maximum number of apps|App ID limit" "$log" >/dev/null; then
    hint="Free Apple IDs can only have a few sideloaded apps at once. Delete one from your device, wait a bit, then run this again."
  elif grep -E "Failed to register bundle identifier|is not available|cannot be registered" "$log" >/dev/null; then
    hint="That app identifier is already taken. Run this again with your own one, like: BUNDLE_ID=com.yourname.mush8 ./build.sh"
  elif grep -E "No space left on device" "$log" >/dev/null; then
    hint="Your Mac ran out of disk space. Free up at least 20 GB, then run this again."
  elif grep -iE "device is locked|passcode|usage assertion" "$log" >/dev/null; then
    hint="Your device is locked. Unlock it, keep it unlocked, then run this again."
  elif grep -E "Developer Mode" "$log" >/dev/null; then
    hint="Turn on Settings > Privacy & Security > Developer Mode on your device, restart it, then run this again."
  elif grep -E "untrusted|not trusted|ApplicationVerificationFailed" "$log" >/dev/null; then
    hint="Unlock your device and tap Trust when it asks to trust this Mac, then run this again."
  elif grep -iE "keys|NCA|decrypt" "$log" >/dev/null; then
    hint="The game files couldn't be read. Check prod.keys matches your dump and the update is version 4.0.1 or 3.0.5."
  fi
  printf '\n\033[31mThe build failed.\033[0m\n'
  if [ -n "$hint" ]; then echo "$hint"; else echo "Last lines of the log:"; tail -15 "$log"; fi
  echo "Full log: $PWD/$log"
  exit 1
}
game_file() {
  local name="$1" what="$2" var="LAST_$3" path
  local last="${!var:-}"
  [ -e "$name" ] && last="${last:-$PWD/$name}"
  while :; do
    ask "$what" "$last"
    path="$(clean_path "$REPLY")"
    if [ -z "$path" ]; then echo "Drag the file into this window, then press Enter."
    elif [ ! -f "$path" ]; then echo "No file at: $path"
    else break
    fi
  done
  path="$(cd "$(dirname "$path")" && pwd)/$(basename "$path")"
  [ "$path" = "$PWD/$name" ] || ln -sf "$path" "$name"
  printf -v "$var" '%s' "$path"
}
remember() {
  mkdir -p build
  { printf 'LAST_KEYS=%q\n' "$LAST_KEYS"; printf 'LAST_BASE=%q\n' "$LAST_BASE"; printf 'LAST_UPDATE=%q\n' "$LAST_UPDATE"; } > "$SAVED"
}

[ "$(uname -s)" = Darwin ] || fail "Mush8 builds on macOS only."
[ "$(uname -m)" = arm64 ] || fail "Mush8 needs an Apple Silicon Mac (M1 or newer)."

bold "Checking Xcode"
if ! xcodebuild -version >/dev/null 2>&1; then
  XCODE="$(ls -d /Applications/Xcode*.app 2>/dev/null | head -1)"
  [ -n "$XCODE" ] || fail "Install Xcode from the App Store (Xcode 26 or later), open it once, then run this again."
  echo "Xcode is installed but not selected. Selecting it (asks for your Mac password):"
  sudo xcode-select -s "$XCODE/Contents/Developer"
fi
XCODE_MAJOR="$(xcodebuild -version | awk '/^Xcode/ {split($2, v, "."); print v[1]}')"
[ "${XCODE_MAJOR:-0}" -ge 26 ] || fail "Xcode $XCODE_MAJOR is too old: update Xcode to 26 or later from the App Store."
if ! xcodebuild -license check >/dev/null 2>&1; then
  echo "Accepting the Xcode license (asks for your Mac password):"
  sudo xcodebuild -license accept
fi
xcodebuild -version | head -1

pick "What do you want to build?" "Mush8 for this Mac" "Mush8 for iPhone / iPad"
PLATFORM="$REPLY"

bold "Checking Homebrew"
if ! command -v brew >/dev/null; then
  [ -x /opt/homebrew/bin/brew ] || /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
  eval "$(/opt/homebrew/bin/brew shellenv)"
fi
brew --version | head -1
command -v cmake >/dev/null || { bold "Installing cmake"; brew install cmake; }
command -v pkg-config >/dev/null || { bold "Installing pkgconf"; brew install pkgconf; }
if [ "$PLATFORM" = 2 ]; then
  command -v xcodegen >/dev/null || { bold "Installing xcodegen"; brew install xcodegen; }
fi

bold "Checking Rust"
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
if ! command -v rustup >/dev/null; then
  brew install rustup
  hash -r
fi
rustup toolchain list | grep '^stable' >/dev/null || rustup toolchain install stable --profile minimal
rustup default >/dev/null 2>&1 || rustup default stable
cargo --version

if [ "$PLATFORM" = 1 ]; then
  bold "Downloading Rust dependencies"
  cargo fetch --locked
  bold "Building Mush8 (a few minutes the first time)"
  run ./make-app.sh
  mkdir -p "$OUT"
  rm -rf "$OUT/Mush8.app"
  ditto build/Mush8.app "$OUT/Mush8.app"
  bold "Done: $OUT/Mush8.app"
  echo "Showing it in Finder: open it, then point it at your keys and game files."
  open -R "$OUT/Mush8.app"
  exit 0
fi

bold "Preparing Metal toolchain"
xcodebuild -downloadComponent MetalToolchain

bold "Preparing the iOS toolchain"
rustup toolchain list | grep "^$TOOLCHAIN" >/dev/null || rustup toolchain install "$TOOLCHAIN" --profile minimal
rustup target list --toolchain "$TOOLCHAIN" --installed | grep '^aarch64-apple-ios$' >/dev/null || rustup target add aarch64-apple-ios --toolchain "$TOOLCHAIN"
if ! xcodebuild -showsdks 2>/dev/null | grep iphoneos >/dev/null; then
  echo "Downloading the iOS platform for Xcode (a few GB):"
  xcodebuild -downloadPlatform iOS
fi
bold "Downloading Rust dependencies"
cargo fetch --locked

LAST_KEYS="" LAST_BASE="" LAST_UPDATE=""
[ -f "$SAVED" ] && . "$SAVED"

bold "Your game files"
echo "The game's code and shaders are prepared from them on this Mac. Drag each file into this window, then press Enter."
game_file prod.keys "prod.keys" KEYS
game_file base.nsp "Mario Kart 8 Deluxe (base game .nsp)" BASE
[ -L upd4-0-1.nsp ] && [ ! -e update.nsp ] && mv upd4-0-1.nsp update.nsp
game_file update.nsp "Update 4.0.1 or 3.0.5 (.nsp)" UPDATE
remember

pick "How do you want to install it?" \
  "Make an .ipa (install it with AltStore or SideStore, works with a free Apple ID)" \
  "Install it on my iPhone / iPad now (plugged into this Mac)" \
  "Just build it"
OUTPUT="$REPLY"

FREE=0
TEAM="${TEAM:-}"
if [ "$OUTPUT" != 1 ] && [ -z "$TEAM" ]; then
  bold "Signing"
  while :; do
    TEAMS="$(xcode_teams)"
    [ -n "$TEAMS" ] && break
    echo "Xcode needs your Apple ID to sign the app (a free one works)."
    echo "Opening Xcode: go to Settings > Accounts, click +, choose Apple ID and sign in."
    open -a Xcode
    read -r -p "Press Enter once you're signed in. " _ || exit 1
  done
  if [ "$(printf '%s\n' "$TEAMS" | wc -l)" -gt 1 ]; then
    NAMES=()
    while IFS=$'\t' read -r _ free name; do
      NAMES+=("$name$([ "$free" = 1 ] && echo " (free)")")
    done <<< "$TEAMS"
    pick "Which account should sign Mush8?" "${NAMES[@]}"
    LINE="$(printf '%s\n' "$TEAMS" | sed -n "${REPLY}p")"
  else
    LINE="$TEAMS"
  fi
  TEAM="$(printf '%s' "$LINE" | cut -f1)"
  FREE="$(printf '%s' "$LINE" | cut -f2)"
  echo "Signing with $(printf '%s' "$LINE" | cut -f3)$([ "$FREE" = 1 ] && echo "'s free account" || echo "'s developer account")."
  if [ "$FREE" = 1 ]; then
    echo "Free accounts don't get the extended address space: Mush8 uses a smaller memory layout for them."
  fi
fi

if [ "$FREE" = 1 ] || [ "$OUTPUT" = 1 ]; then
  BUNDLE="${BUNDLE_ID:-com.$(id -un | tr -cd '[:alnum:]' | tr '[:upper:]' '[:lower:]').mush8}"
else
  BUNDLE="${BUNDLE_ID:-ch.cclerc.mush8}"
fi

FREE_KB="$(df -k . | awk 'NR==2 {print $4}')"
[ "$FREE_KB" -gt $((20 * 1024 * 1024)) ] || echo "Warning: less than 20 GB free on this disk, the build may run out of space."

if [ "$OUTPUT" = 2 ]; then
  while ! xcrun devicectl list devices 2>/dev/null | grep -E 'iPhone|iPad' | grep -E 'available \(paired\)|connected' >/dev/null; do
    echo
    echo "No iPhone / iPad found. Plug it in with a cable, unlock it, tap Trust,"
    echo "and turn on Settings > Privacy & Security > Developer Mode."
    read -r -p "Press Enter to look again. " _ || exit 1
  done
fi

bold "Building Mush8 for iOS (20-40 minutes the first time, mostly shaders)"
case "$OUTPUT" in
  1)
    BUNDLE_ID="$BUNDLE" run ./make-ios.sh --ipa
    mkdir -p "$OUT"
    cp build/Mush8.ipa "$OUT/Mush8.ipa"
    bold "Done: $OUT/Mush8.ipa"
    echo "Install it with AltStore or SideStore, then copy prod.keys, base.nsp and the update .nsp"
    echo "(and optionally dlc.nsp) into Files > On My iPhone > Mush8."
    open -R "$OUT/Mush8.ipa"
    ;;
  2)
    TEAM="$TEAM" BUNDLE_ID="$BUNDLE" FREE="$FREE" run ./make-ios.sh --install
    bold "Done: Mush8 is on your device"
    echo "Copy prod.keys, base.nsp and the update .nsp (and optionally dlc.nsp) into its folder:"
    echo "Finder > your device > Files > Mush8."
    echo "If it says \"Untrusted Developer\": Settings > General > VPN & Device Management, tap your account, Trust."
    ;;
  *)
    TEAM="$TEAM" BUNDLE_ID="$BUNDLE" FREE="$FREE" run ./make-ios.sh
    bold "Done"
    grep '^built:' build/build.log | tail -1
    ;;
esac
