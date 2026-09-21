# OPhim source (`vi.ophim`)

Scrapes the **OPhim API** (`https://ophim1.com`) and its family of clones
(`https://phimapi.com` …) for the Komorei app. Handles **both envelope
shapes**: the classic `{ status, data: { items | item, params } }` and the flat
fork `{ status, items | movie }`.

> ⚠️ OPhim domains die and are reborn constantly. At the time of writing
> (09/2026), `ophim1.com` and every classic mirror are unreachable; `phimapi.com`
> is still up but is a **stub fork** — search/list/detail metadata work, but it
> does **not return `episodes[].server_data[].link_m3u8`** (nothing to play). So
> the base URL must be configurable: default `https://ophim1.com`, change it in
> **Source settings → "OPhim API address"** (setting `base_url`, re-read on every
> request via `defaults_get`, no caching).

## Data model → Komorei

| Komorei | OPhim |
|---|---|
| `Anime` (lite) | item in a list/search |
| `Anime` (full) | detail `GET {base}/phim/{slug}` |
| `AnimeSeason` | **one playback server** on the detail (`episodes[].server_name`) |
| `Episode` | entry in `server_data` — `key = slug` (e.g. `tap-1`) |

- **Season = playback server.** `AnimeSeason.anime_id` is encoded as
  `"{slug}|{server_name}"`. The app calls `get_anime_update` with that key (via
  `fetchEpisodesForSeason`) and the source strips everything after `|` to return
  the episodes of the selected server. Servers are ordered by episode count
  descending, so the largest one (usually `OPhim`) is the default.
- **`get_stream_list`** returns 1 `StreamInfo` per server (taken from the
  seasons just received, **no extra request**).
- **`get_stream`** refetches `{base}/phim/{slug}`, finds the group by
  `stream.key` (server) + the entry by `episode.key`; if the requested server
  has no such episode (the app auto-resolves the first server) it **falls back
  to the first group that contains the episode**. Returns `StreamData` with
  `is_content: false` (plain media URI) + header `Referer: {base}/`.
- **Lists/pagination**: `danh-sach` (list) and `tim-kiem` (search) both read
  `data.items`/`items` + `params.pagination`; when the fork omits pagination,
  `has_next` falls back to a 24-items-per-page threshold.
- **Filters**: genre/country name → OPhim slug via the `GENRES`/`COUNTRIES`
  tables (simple slugify for unknown ones); type `series|single`; year; sort
  `modified.time|year|name`.

## URLs

| Action | URL |
|---|---|
| Home (3 parallel rails) | `GET {base}/danh-sach/{phim-moi-cap-nhat,phim-bo,phim-le}?page=1` |
| Browse (no keyword) | `GET {base}/danh-sach/phim-moi-cap-nhat?page=N&filters…` |
| Search (with keyword) | `GET {base}/tim-kiem?page=N&keyword=…&filters…` |
| Detail | `GET {base}/phim/{slug}` |
| Deep link | `/phim/{slug}` → Anime · `/xem-phim/{slug}/{ep-slug}` → Episode |

## Build, package & test

```sh
# wasm + package.krx (see also sources/README.md)
cargo build --release --target wasm32-unknown-unknown

# pure host unit tests (no komorei-test-runner needed):
# --target x86_64-unknown-linux-gnu guards against build.target = wasm in config.
cargo test --target x86_64-unknown-linux-gnu

# JVM integration test running the real WASM through the runner + real host,
# against a local classic-OPHIM fixture (no network needed):
./gradlew :app:testDebugUnitTest \
  --tests "git.shin.komorei.sdk.OphimSourceRunnerIntegrationTest"
```

The 16-test suite covers: search (browse/search, both envelopes), home rails,
detail + seasons-per-server, chapters scoped by the `{slug}|{server}` key,
stream list/resolve, server fallback + "no episode" bail, filters, settings
(`base_url`), listings, deep links, notification, migration identity. (Plus 9
host-side Rust unit tests.)

## Implementation notes

- `MigrationHandler` stays identity (`OPhim` does not change ids across
  versions).
- `parse_isodate_millis` (Hinnant `days_from_civil`) → epoch millis from
  `modified.time` (the app reads `Instant.ofEpochMilli`).
- `strip_html` removes tags and a few HTML entities from `content` to build the
  description.