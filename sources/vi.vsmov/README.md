# VSMov source (`vi.vsmov`)

Scrapes the **VSMov API** (`https://vsmov.com/api` — "Nguồn API Phim Miễn Phí")
for the Komorei app. The API is an **OPhim-family flat fork**: lists, search,
genre/country detail pages and the movie detail all return their payload at the
ROOT of the envelope (`{ status, items, pagination }` / `{ status, movie,
episodes }`), while the genre/country **catalogs** keep the classic
`{ status: "success", data: { items } }` shape.

## Endpoints

| Purpose | Path (relative to `{base}`) |
|---|---|
| Recently updated / browse | `/danh-sach/phim-moi-cap-nhat?page=N&category=..&country=..&year=..&type=..&sort_field=..&sort_type=..` |
| Listings | `/danh-sach/{phim-bo,phim-le,phim-chieu-rap,subteam}?page=N` |
| Search | `/tim-kiem?keyword=X&page=N&…` (⚠️ breaks on an empty `keyword`) |
| Detail | `/phim/{slug}` |
| Genre catalog / detail | `/the-loai` / `/the-loai/{slug}?page=N` |
| Country catalog / detail | `/quoc-gia` / `/quoc-gia/{slug}?page=N` |

## Data model → Komorei

| Komorei | VSMov |
|---|---|
| `Anime` (lite) | item in a list/search — `_id` is an int, `poster_url` may be `{}` |
| `Anime` (full) | detail `GET {base}/phim/{slug}` |
| `AnimeSeason` | **one playback server** (`episodes[].server_name`) — `anime_id = "{slug}\|{server}"` |
| `Episode` | entry in `server_data` — `key = slug` (`tap-1`, ...) |

## Streams & the PNG-header segments

The API exposes **only** `link_embed = https://vX.streamvsmov.com/video/<hash>`
(the JW embed page). The source derives the real HLS playlist from it —
`https://vX.streamvsmov.com/stream/<hash>/master.m3u8` — which is exactly what
the embed page requests itself. The playlist's segments are `.png`-named files:
the first ~633 bytes are a decoy PNG header followed by a **fully valid MPEG-TS
stream** (verified: H.264 1920×800 + AAC 48 kHz per segment). ExoPlayer's
`TsExtractor` scans past the preamble for the `0x47` sync byte, so Media3 plays
them natively — no segment unwrapping.

## Subtitles

vsmov keeps real external **`.vtt` subtitle tracks** per episode
(`/video/<hash>/subtitle/vie_*.vtt`, `eng_*.vtt`) exposed by the embed page's
`playerOptions.subtitles`. `get_stream` fetches the embed page, reads the array
with a small JSON-object scanner and publishes each track as
`StreamData.subtitles`. The "bỏ qua giới thiệu" button on the site is
**user-configured** (localStorage) — no server-provided intro/outro exists, so
`intro`/`outro` are left unset.

> ⚠️ The backend gates stream quality by IP: on some networks the playlist
> returns the decoy PNG segments for *every* movie; on the user's residential
> device the same URLs serve real video. Stream handling is therefore verified
> structurally (embed derivation + valid TS segments) but not via a real
> residential playback session from CI.