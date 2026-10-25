# rmp — TUI music player (Rust)

ratatui + rodio/symphonia. Local files with real tags, YouTube/SoundCloud through `yt-dlp`,
Discord Rich Presence, FFT visualizer, queue, shuffle/repeat, playlists, matugen theming and
real cover art in kitty.

## Requirements
- Rust (`cargo run --release`), on Linux also `libasound2-dev` / `alsa-lib`
- `yt-dlp` + `ffmpeg` in PATH (for YouTube/SoundCloud and their covers)
- kitty (or another terminal supporting the kitty graphics protocol) for the image wave/cover

## Install
```sh
cargo build --release
install -m755 target/release/rmp ~/.cargo/bin/rmp   # or ~/.local/bin
```

## CLI
```sh
rmp                          # start the player
rmp --version
rmp --help
rmp --print-config           # path + effective config
rmp --preview-wave 1.0       # write wave previews to $TMPDIR (tune stroke weight)
rmp --rpc-test               # show a Discord test activity for 8 seconds
rmp --dump-library [DIR]     # print scanned tracks, tags and cover presence
rmp --embed-cover <audio>    # embed the <stem>.png/jpg next to a file into its tags
rmp --setup-matugen ~/wall.jpg
```

## Config
`~/.config/rmp/config.json` is created on first run and is rewritten with any missing keys:

| key | default | meaning |
|---|---|---|
| `music_dir` | `$XDG_MUSIC_DIR` | library folder (scanned recursively) |
| `cache_dir` | `~/.cache/rmp` | streamed tracks and covers |
| `discord_app_id` | `1561845271334486016` | Discord application ID (empty = RPC off) |
| `discord_image_key` | `logo` | fallback large art asset when a track has no artwork URL |
| `discord_small_key` | `""` | optional small badge asset shown while paused |
| `discord_buttons` | `true` | "Open track" button for remote tracks |
| `ytdlp` | `yt-dlp` | binary used for search/download |
| `volume` | `0.7` | saved on change |
| `viz` | `wave` | `wave`, `bars`, `mirror`, `spectrum` |
| `graphics` | `auto` | `auto` = kitty graphics when available, `off` = cells only |
| `wave_scale` | `1.0` | wave stroke thickness (`[` / `]` in the app) |
| `theme` | `matugen` | `matugen` follows `colors.json`, `builtin` uses `accent` |
| `accent` | `#7fd6be` | accent color for the builtin theme |
| `provider` | `youtube` | last search source |

Volume, visualiser, source, wave scale and graphics are persisted automatically (debounced).

## Discord
The Rich Presence application ID `1561845271334486016` is the default. To use your own:
1. Create an app at https://discord.com/developers/applications
2. Put its Application ID into `discord_app_id`
3. Upload a Rich Presence art asset and name it like `discord_image_key` (default `logo`)
4. Optional: run `rmp --rpc-test` and check your Discord profile

The activity is sent as `Listening` with the track title on the title line (the payload's
`name` field; if a Discord build ignores it, the title is still in `details`), `artist · album`
in `state`, a progress bar from the track position/duration, an "Open track" button for
YouTube/SoundCloud, and a "paused" badge when paused.

The per-track cover is sent as `large_image` when a public https artwork URL is known (search
results, and downloaded tracks thanks to the `<stem>.url` note written at download time).
Discord cannot read a local file, so a purely local rip without a URL falls back to
`discord_image_key`; the cover is still shown inside rmp. The IPC client has I/O timeouts, so a
hung Discord cannot freeze the player, and it reconnects automatically (the presence is also
refreshed every 15 s, which revives it after a Discord restart).

## Tags and covers
- `lofty` reads title/artist/album/duration from the file tags; file names (`Artist - Title`)
  are the fallback.
- Cover art comes from the embedded picture, or `<track stem>.png/jpg/webp` next to the file,
  then `cover.*` / `folder.*` / `front.*` / `album.*`, or the YouTube/SoundCloud thumbnail for
  remote tracks.
- Every download (`D` in the app, or streamed tracks in the cache) also fetches the artwork,
  normalises it to a 512 px `<stem>.png` sidecar, embeds it into the audio tags and remembers
  the public artwork URL in `<stem>.url` for Discord. Existing files can be fixed with
  `rmp --embed-cover <audio>`.
- Covers are converted to a 512 px PNG in `cache_dir/covers/` and shown in the Now Playing block
  through kitty graphics. Other terminals keep the generated placeholder.

## matugen
```sh
rmp --setup-matugen ~/Pictures/wall.jpg
```
This installs `~/.config/matugen/templates/rmp-colors.json`, adds the required `[config]` table
and `[templates.rmp]` block to `~/.config/matugen/config.toml` (absolute paths), runs
`matugen image -m dark` and checks the result. rmp reloads `~/.config/rmp/colors.json` within a
second, so re-running matugen re-themes a running player.

The palette keys are `accent`, `secondary`, `surface`, `surface_hi`, `background`, `on_surface`,
`on_surface_variant`, `outline`, `error`. Manual setup is still possible with
`extras/matugen/config.toml.snippet`.

## Keys
| key | action |
|---|---|
| Space | pause / resume |
| n / p | next / previous (p restarts if >3s in) |
| ← / → | seek -5s / +5s |
| + / - | volume |
| s / r | shuffle / repeat (off → all → one) |
| 1-6, Tab | Library / Queue / Search / Downloads / Playlists / Settings |
| v | switch visualiser: wave · bars · mirror · spectrum |
| [ / ] | wave stroke thinner / thicker |
| ? | help overlay |
| j k g G | move selection |
| / | search (Enter to run, Esc to cancel) |
| P | switch YouTube / SoundCloud |
| Enter | play (list becomes the queue) / jump in queue |
| a | add to queue |
| d | remove from queue (keeps playing: advances to the next track) |
| D | download selected remote track into music_dir |
| w | save queue as a named playlist (Playlists tab: Enter loads, d deletes) |
| q | quit |

## Layout
- `player.rs` rodio sink, seek, sample tap for the visualizer
- `visualizer.rs` 1024-pt FFT → 64 log bands
- `queue.rs` order/shuffle/repeat and current-track removal (with tests)
- `library.rs` file scan + tag reading, `covers.rs` cover extraction/download
- `ytdlp.rs` search + download, `discord.rs` RPC client
- `theme.rs` palette, `matugen.rs` template/setup
- `kittyviz.rs` kitty graphics: DPI-aware wave rendering and cover placement
- `app.rs` state + keys, `ui.rs` rendering

## Known limits / next steps
- Remote tracks are downloaded to cache first, then played (not true streaming yet)
- No MPRIS/media-key integration yet
- Search returns 15 results per query (no paging)
- Cache is not evicted automatically

## Mouse
Click a tab to switch, click a row to select, double-click to play/load. Click on the progress bar
to seek. Wheel scrolls the list, or changes volume elsewhere. To select text in the terminal, hold
Shift while dragging.

## Tests
```sh
cargo test
```

## Layouts
Press `T` (or Settings → Layout) to cycle presets:
- **resonance**: tabs on top, list left, visualiser and now playing on the right (default).
- **minimal**: one big list and a two-row now-playing strip, no visualiser.
- **focus**: large visualiser on top, list and a large now-playing card below.
The choice is saved to `config.json` as `"layout"`.
