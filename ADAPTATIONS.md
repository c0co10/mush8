# Apple-device adaptations for MK8 Deluxe 4.0.1

This feature branch contains the source adaptations developed against Mush8's
initial commit (`1b2e9b8`), rebased onto `975040b` (firmware requirement checks).
It retains the upstream GPL-3.0 license and author notices.

## Changes

- Optional firmware-free operation: local SFNT shared-font fallback, missing
  system-data errors at the SDK API boundary, a constructed empty Mii avatar,
  and guards for missing Mii layouts, thumbnails and expressions in LAN lobbies.
- A version-checked local Player Mii parameter record for fresh account setup.
- Extracted base Program NCA input through `--base-nca`, alongside NSP input.
- Native aspect adaptation for world cameras and outermost UI panes, preserving
  character proportions; Mac windows can resize and enter fullscreen.
- GPU heap mappings split at Metal's `maxBufferLength`, with cross-chunk vertex,
  index, uniform and copy handling. Common one/two-chunk shader paths avoid
  runtime 64-bit division, and shader caches use a revised resource ABI.
- Focused Mac keyboard input, cleared when focus is lost.
- Persistent iOS game-language selection, including system language.
- An embedded Mac settings window for local app bundles: language, phone-pad
  hosting and off/LAN/Wireless. It reuses upstream phone and wireless transports.

The missing-resource and camera hooks are specific to MK8 Deluxe **4.0.1**, main
build ID `158c0c7399aabd7bf83795c2ef3484c700000000000000000000000000000000`.
Build-ID and instruction checks reject an unexpected executable; do not reuse
these offsets for another game or game version.

## Firmware-free limits

No Mii model, face texture or Mii editor is supplied. The compatibility path
keeps an empty, constructed avatar and normal job completion, rather than
recreating Nintendo's resources. Mii graphics are absent. Local font substitution
can differ from the original game's typography and symbol coverage.

Normal upstream builds still use your own firmware. To use this branch without
it, provide a local font and, for iOS, explicitly enable the baked no-Mii profile.
A signed iOS no-Mii binary keeps that profile even if firmware is later imported;
rebuild without the profile to use real Mii rendering. This branch also keeps
upstream's non-empty-folder and directory-import fixes.

## Prepare a local fallback font

Use a font available on your own machine. The helper creates a local BMP subset;
it does not download or distribute fonts.

```sh
python3 -m venv .venv
.venv/bin/python -m pip install fonttools
.venv/bin/python tools/prepare_host_font.py
```

The default input is macOS's installed Arial Unicode font and the output is
`work/host-font.ttf`. If that font is unavailable, supply `--source /path/to/font.ttf`.
Keep the generated font and any app embedding it local.

## Mac

Prepare your own `prod.keys`, `base.nsp`, `upd4-0-1.nsp` and optional `dlc.nsp` in
a local game directory. From this repository, extract and rewrite the modules
using upstream `nxfs` and `nxpatch`, then create a local app:

```sh
cargo build --locked --release
# Run these from the game directory (substitute your checkout path):
/path/to/mush8/target/release/nxfs extract --base base.nsp --update upd4-0-1.nsp --out work/v-upd
/path/to/mush8/target/release/nxpatch rewrite --out work/v-upd/patched work/v-upd/exefs/main work/v-upd/exefs/subsdk0 work/v-upd/exefs/sdk
# Return to the repository:
python3 tools/make_local_app.py --game-dir /path/to/game \
  --patched /path/to/game/work/v-upd/patched \
  --npdm /path/to/game/work/v-upd/exefs/main.npdm --network wireless
```

This creates `build/MK8 Native.app`. It stores only the compiled host and private
path configuration in that local bundle. The original game/key files stay at
the supplied locations. The app starts with an embedded settings window; choose
settings and start the game. `--network off` and `--network lan` are also supported.
Use `--base-nca /path/to/base-program.nca` when you already extracted that NCA;
this does not itself extract an XCI. The embedded settings window defaults phone
controllers on. Upstream `make-app.sh` and its original launcher remain available
for the normal firmware-backed setup.

## iOS

Follow upstream prerequisites and prepare the game files in the repository's
local game-data directory as expected by `make-ios.sh`. Generate the font above,
then build with your own signing configuration:

```sh
FIRMWARE_FREE=1 TEAM=YOUR_TEAM_ID BUNDLE_ID=your.own.bundle.id ./make-ios.sh --install
```

`FIRMWARE_FREE=1` sets `NX_MISSING_MII=1` before compiling the iOS executable and
bundles `work/host-font.ttf` under `fallback/`. `NX_FALLBACK_FONT` can select another
local font. The game instructions and all no-Mii/camera redirects are baked before
signing; no executable-memory writes are performed on iOS. The generated signed
app contains your game-derived executable sections: keep it local, outside Git.

The iOS launcher accepts `base.nca` or `base.nsp`. Update installs retain the
application's game files and configuration. Choose **Game language** on the
launcher before Play; this changes the game language, not all launcher labels.

## Phone controller and Wireless

For a phone controller, start the Mac game with phone controllers enabled and
choose **Use as a controller for a Mac** on the iPhone launcher. Use the same
Wi-Fi. A phone uses the first available player slot; keyboard input still merges
into player one. Controller mode does not require game data on the phone.

For independent multiplayer, each device needs game data. Set both launchers to
Wireless (or both to LAN) and use the corresponding network entry in the game's
main menu. Keep the iPhone app foreground and the screen awake. iOS backgrounding
pauses the guest and can cause a connection timeout; rejoin after a disconnection.

Local testing confirmed Mac/iPhone room joins, race entry and vehicle movement
synchronization on the same Wi-Fi. Router-free AWDL, Switch interoperability and
full-length race endurance were not verified. These are limited smoke tests,
not a claim of complete game compatibility.

## Validation

The original integrated build was exercised on a Mac, iPhone 15 Pro and iPhone
13 Pro. Source-branch build and automated verification are recorded separately
in `VALIDATION.md`.

```sh
cargo test --locked --release -p nxhost -p nxshader
cargo test --locked --release -p nxhost gpu_reads_and_writes_across_heap_chunks -- --ignored --exact nvn::draw::state::segmented_heap_tests::gpu_reads_and_writes_across_heap_chunks
# Local iOS executable only; requires Python unicorn for the second command:
python3 tools/verify_display_hooks.py /path/to/ios/nxhost --report /tmp/display-check.json
python3 tools/verify_lan_avatar_guards.py /path/to/ios/nxhost --report /tmp/avatar-check.json
```

No game archives, keys, firmware, extracted modules, fonts, shader caches, signed
apps, provisioning profiles, private logs or machine-specific launch.json files
are included in this branch.
