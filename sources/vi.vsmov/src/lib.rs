//! # VSMov source (`vi.vsmov`)
//!
//! Scrapes the **VSMov API** (`https://vsmov.com/api`) — "Nguồn API Phim Miễn
//! Phí" — for the Komorei app. The API is an OPhim-family *flat* fork (like
//! phimapi.com): lists/search/detail return their payload at the ROOT of the
//! envelope (no `data` wrapper), but the genre/country CATALOGS keep the
//! classic `{ status: "success", data: { items } }` shape. Envelope summary:
//!
//! - list / search / genre-detail / country-detail:
//!   `{ status: true, items: [...], pagination: { currentPage, totalPages, ... } }`
//! - detail: `{ status: true, msg, movie: {...}, episodes: [{ server_name,
//!   server_data: [{ name, slug, filename, link_embed }] }] }` — the episodes
//!   carry **only** `link_embed` (no `link_m3u8`).
//! - genre/country catalogs: `{ status: "success", data: { items: [{name, slug}] } }`
//!
//! ## Data model mapping to Komorei
//!
//! - **Season = one playback server** on the detail page. `AnimeSeason.anime_id`
//!   is encoded as `"{slug}|{server_name}"`; `get_anime_update` splits it and
//!   returns the episodes of the selected server (largest one first, usually
//!   `Vietsub`). This mirrors `vi.ophim` exactly.
//! - **Episode key = `slug`** of the entry (`tap-1`, ...).
//! - `get_stream_list`: one `StreamInfo` per server of the just-received season
//!   list. `get_stream`: refetches the detail to locate the entry, then resolves
//!   the playable HLS from the embed page.
//!
//! ## Stream resolution & the PNG-header segments
//!
//! The API only exposes `link_embed = https://vX.streamvsmov.com/video/<hash>`
//! (the JW-embed page). The embed page's own player requests
//! `https://vX.streamvsmov.com/stream/<hash>/master.m3u8`, so this source
//! derives that playlist URL directly (`embed_to_m3u8`) and hands it to the
//! app's Media3 engine. The playlist segments are `.png`-named files whose
//! first ~633 bytes are a decoy PNG header (IHDR/pHYs/IDAT/IEND) followed by a
//! **fully valid MPEG-TS stream** — ExoPlayer's `TsExtractor` scans for the
//! `0x47` sync byte past the preamble and plays them natively (verified with
//! ffprobe: H.264 1920×800 + AAC 48 kHz per segment). No segment unwrapping is
//! needed.
//!
//! ## Subtitles
//!
//! vsmov keeps real **external `.vtt` subtitle tracks** per episode
//! (`/video/<hash>/subtitle/vie_*.vtt`, `eng_*.vtt`). They are not derivable —
//! the filenames embed a per-upload timestamp. `get_stream` therefore fetches
//! the embed page once and reads the `playerOptions.subtitles` array (a flat
//! `{name, type, url, code, ...}` list) with a small JSON-object scanner, then
//! exposes each track as `StreamData.subtitles`. Intro/outro markers (`RangeLong`)
//! are intentionally NOT populated: the site's "bỏ qua giới thiệu" button is a
//! **user-configured** localStorage skip (`BC_INTRO_SKIP_KEY`), not server data.

#![no_std]
extern crate alloc;

use alloc::{
	borrow::Cow,
	format,
	string::{String, ToString},
	vec,
	vec::Vec,
};
use komorei::{
	Anime, AnimePageResult, AnimeSeason, AnimeStatus, AnimeWithEpisode, ButtonSetting,
	CategoryLink, DeepLinkHandler, DeepLinkResult, DynamicFilters, DynamicListings,
	DynamicSettings, Episode, Filter, FilterItem, FilterValue, Home, HomeComponent,
	HomeComponentValue, HomeLayout, Link, LinkValue, Listing, ListingKind, ListingProvider,
	MigrationHandler, MultiSelectFilter, NotificationHandler, RangeFilter, Result, SelectFilter,
	Setting, SortFilter, SortFilterDefault, Source, StreamData, StreamInfo, StreamType, SubtitleInfo,
	TextFilter, TextSetting,
	helpers::uri::QueryParameters,
	imports::defaults::{DefaultValue, defaults_get, defaults_set},
	imports::net::Request,
	prelude::*,
	serde::Deserialize,
};

const SOURCE_ID: &str = "vi.vsmov";
const DEFAULT_BASE: &str = "https://vsmov.com/api";
/// Items per API page when there is no `pagination` (fallback for has_next).
const ITEMS_PER_PAGE: usize = 24;
const SETTING_BASE_URL: &str = "base_url";
const SETTING_LAST_NOTIFICATION: &str = "last_notification";

// ────────────────────────────────────────────────────────────────────────────
// Genre & Country catalogs (name → vsmov slug, lifted from `/api/the-loai` and
// `/api/quoc-gia`). Used for both the home "Thể Loại" chips and the
// MultiSelect filters — when building the search URL the name is mapped to a
// slug through these tables, so the filter options and the applied slugs always
// agree even for diacritic-heavy Vietnamese names.
// ────────────────────────────────────────────────────────────────────────────

const GENRES: &[(&str, &str)] = &[
	("Action & Adventure", "action-adventure"),
	("Bí Ẩn", "bi-an"),
	("Chiến Tranh", "chien-tranh"),
	("Chính Kịch", "chinh-kich"),
	("Chính Trị - Chiến Tranh", "chinh-tri-chien-tranh"),
	("Chủ Đề Thực Tế", "chu-de-thuc-te"),
	("Cổ Trang", "co-trang"),
	("Drama", "drama"),
	("Gia Đình", "gia-dinh"),
	("Giả Tưởng", "gia-tuong"),
	("Giật Gân", "giat-gan"),
	("Hài", "hai"),
	("Hành Động", "hanh-dong"),
	("Hình Sự", "hinh-su"),
	("Hoạt Hình", "hoat-hinh"),
	("Học Đường", "hoc-duong"),
	("Hôn Nhân", "hon-nhan"),
	("Khoa Học Viễn Tưởng", "khoa-hoc-vien-tuong"),
	("Kiếm Hiệp", "kiem-hiep"),
	("Kinh Dị", "kinh-di"),
	("Lãng Mạn", "lang-man"),
	("Phiêu Lưu", "phieu-luu"),
	("Sci-Fi & Fantasy", "sci-fi-fantasy"),
	("Thanh Xuân", "thanh-xuan"),
	("Thiếu Nhi", "thieu-nhi"),
	("Thương Trường", "thuong-truong"),
	("Tiên Hiệp", "tien-hiep"),
	("Tình Cảm Gia Đình", "tinh-cam-gia-dinh"),
	("Tội Phạm", "toi-pham"),
	("Truyền Hình Thực Tế", "truyen-hinh-thuc-te"),
	("Viễn Tưởng", "vien-tuong"),
	("Võ Hiệp", "vo-hiep"),
	("Võ Thuật", "vo-thuat"),
];

const COUNTRIES: &[(&str, &str)] = &[
	("Hàn Quốc", "han-quoc"),
	("Nhật Bản", "nhat-ban"),
	("Mỹ", "my"),
	("Âu Mỹ", "au-my"),
	("Anh", "anh"),
	("Pháp", "phap"),
	("Đức", "duc"),
	("Trung Quốc", "trung-quoc"),
	("Hồng Kông", "hong-kong"),
	("Thái Lan", "thai-lan"),
	("Đài Loan", "dai-loan"),
	("Ấn Độ", "an-do"),
	("Tây Ban Nha", "tay-ban-nha"),
	("Úc", "uc"),
	("Nga", "nga"),
	("Canada", "canada"),
	("Ý", "y"),
	("Thụy Điển", "thuy-dien"),
	("Việt Nam", "viet-nam"),
];

// ────────────────────────────────────────────────────────────────────────────
// JSON structs — the flat vsmov envelopes.
// ────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct NameSlug {
	name: String,
	slug: String,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Modified {
	time: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct EpisodeEntry {
	name: String,
	slug: String,
	filename: Option<String>,
	/// vsmov exposes ONLY the embed page — no `link_m3u8` field.
	link_embed: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct EpisodeGroup {
	server_name: String,
	server_data: Vec<EpisodeEntry>,
}

/// `episode_total`/`episode_current` — strings on vsmov, but some OPhim forks
/// send raw integers; accept both so a drift never kills the whole detail.
#[derive(Deserialize)]
#[serde(untagged)]
enum StrOrNum {
	Text(String),
	Number(i64),
}

fn de_opt_str_or_num<'de, D>(de: D) -> core::result::Result<Option<String>, D::Error>
where
	D: serde::Deserializer<'de>,
{
	Ok(match Option::<StrOrNum>::deserialize(de)? {
		Some(StrOrNum::Text(s)) => Some(s),
		Some(StrOrNum::Number(n)) => Some(n.to_string()),
		None => None,
	})
}

/// vsmov sends `poster_url`/`thumb_url` as an absolute URL **or an empty object**
/// (`{}`) for films without a poster (it can also be a JSON object with a
/// single `url` key on some CDN hosts). Untagged: any non-string value is a
/// "no poster".
#[derive(Deserialize)]
#[serde(untagged)]
enum PicUrl {
	Url(String),
	NotAString(serde::de::IgnoredAny),
}

fn de_opt_pic<'de, D>(de: D) -> core::result::Result<Option<String>, D::Error>
where
	D: serde::Deserializer<'de>,
{
	Ok(match Option::<PicUrl>::deserialize(de)? {
		Some(PicUrl::Url(s)) => Some(s),
		_ => None,
	})
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct VsmovMovie {
	name: String,
	origin_name: String,
	slug: String,
	#[serde(default, deserialize_with = "de_opt_pic")]
	poster_url: Option<String>,
	#[serde(default, deserialize_with = "de_opt_pic")]
	thumb_url: Option<String>,
	year: Option<i32>,
	#[serde(rename = "type")]
	r#type: Option<String>,
	quality: Option<String>,
	lang: Option<String>,
	#[serde(default, deserialize_with = "de_opt_str_or_num")]
	episode_total: Option<String>,
	#[serde(default, deserialize_with = "de_opt_str_or_num")]
	episode_current: Option<String>,
	status: Option<String>,
	content: Option<String>,
	director: Vec<String>,
	actor: Vec<String>,
	category: Vec<NameSlug>,
	country: Vec<NameSlug>,
	modified: Option<Modified>,
	view: Option<i32>,
	episodes: Vec<EpisodeGroup>,
}

/// List/search/genre-detail/country-detail envelope — payload at the ROOT.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct ListEnvelope {
	items: Option<Vec<VsmovMovie>>,
	pagination: Option<Pagination>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Pagination {
	#[serde(rename = "currentPage")]
	current_page: i32,
	#[serde(rename = "totalPages")]
	total_pages: i32,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct DetailEnvelope {
	status: Option<bool>,
	msg: Option<String>,
	movie: Option<VsmovMovie>,
	episodes: Option<Vec<EpisodeGroup>>,
}

/// Genre/country CATALOG — the classic `{ status: "success", data: { items } }`.
/// Runtime never fetches it (the GENRES/COUNTRIES tables are hard-coded from
/// today's catalog), so these only exist to keep the envelope shape documented
/// and tested.
#[cfg(test)]
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct CatalogEnvelope {
	data: Option<CatalogData>,
}

#[cfg(test)]
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct CatalogData {
	items: Vec<NameSlug>,
}

// ────────────────────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────────────────────

/// Extract the server from the season key `"{slug}|{server_name}"`.
fn split_server(key: &str) -> (&str, Option<&str>) {
	match key.find('|') {
		Some(i) => (&key[..i], Some(&key[i + 1..])),
		None => (key, None),
	}
}

/// Fold whitespace runs (including the `\r\n ` the site appends to server
/// labels, e.g. `"Vietsub\r\n #1"`) into a single space and trim: the label
/// becomes `"Vietsub #1"`. Applied consistently wherever server names are
/// built or compared, so season titles and stream lookups always agree.
fn normalize_server_name(name: &str) -> String {
	let mut out = String::with_capacity(name.len());
	let mut prev_space = true;
	for c in name.trim().chars() {
		if c.is_whitespace() {
			if !prev_space {
				out.push(' ');
				prev_space = true;
			}
		} else {
			out.push(c);
			prev_space = false;
		}
	}
	out
}

/// Origin of the base URL — used to absolutize protocol-relative/relative
/// image paths against the SITE host (the base may be `/api`, the images live
/// on the root).
fn base_origin(base: &str) -> String {
	if let Some(rest) = base.strip_prefix("https://").or_else(|| base.strip_prefix("http://")) {
		match rest.find('/') {
			Some(i) => format!("https://{}", &rest[..i]),
			None => format!("https://{rest}"),
		}
	} else {
		base.to_string()
	}
}

/// Turn a protocol-relative (`//vsmov.com/...`) or relative path URL absolute.
fn absolutize_url(url: &str, origin: &str) -> String {
	if url.starts_with("//") {
		format!("https:{url}")
	} else if url.starts_with('/') {
		format!("{origin}{url}")
	} else {
		url.to_string()
	}
}

fn absolutize_opt(url: Option<&str>, origin: &str) -> Option<String> {
	url.map(|u| absolutize_url(u, origin)).filter(|s| !s.is_empty())
}

/// `https://<host>/video/<hash>` — the only embed field vsmov exposes — →
/// `https://<host>/stream/<hash>/master.m3u8` (the HLS playlist the embed page
/// itself requests). URLs already ending in `.m3u8` pass through untouched.
fn embed_to_m3u8(url: &str) -> Option<String> {
	let url = url.trim();
	if url.contains(".m3u8") {
		return Some(url.to_string());
	}
	let rest = url
		.strip_prefix("https://")
		.or_else(|| url.strip_prefix("http://"))?;
	let vid = rest.find("/video/")?;
	let host = &rest[..vid];
	let hash = &rest[vid + "/video/".len()..];
	if host.is_empty() || hash.is_empty() {
		return None;
	}
	Some(format!("https://{host}/stream/{hash}/master.m3u8"))
}

/// Origin (`https://<host>`) of an embed URL — used to absolutize the relative
/// `.vtt` subtitle paths the embed page advertises.
fn embed_host(url: &str) -> Option<String> {
	let rest = url
		.trim()
		.strip_prefix("https://")
		.or_else(|| url.strip_prefix("http://"))?;
	let end = rest.find('/').unwrap_or(rest.len());
	let host = &rest[..end];
	if host.is_empty() {
		return None;
	}
	Some(format!("https://{host}"))
}

/// Extract the first run of digits in a string ("Tập 12/24" → "12", "Full" → "1").
fn parse_episode_number(name: &str) -> String {
	let mut digits = String::new();
	for c in name.chars() {
		if c.is_ascii_digit() {
			digits.push(c);
		} else if !digits.is_empty() {
			break;
		}
	}
	if digits.is_empty() {
		String::from("1")
	} else {
		digits
	}
}

/// Extract the first run of digits as an integer.
fn parse_first_int(input: Option<&str>) -> Option<i32> {
	let input = input?;
	let mut digits = String::new();
	for c in input.chars() {
		if c.is_ascii_digit() {
			digits.push(c);
		} else if !digits.is_empty() {
			break;
		}
	}
	digits.parse().ok()
}

/// Howard Hinnant `days_from_civil` — civil date → days since the epoch.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
	let y = if m <= 2 { y - 1 } else { y };
	let era = if y >= 0 { y } else { y - 399 } / 400;
	let yoe = y - era * 400;
	let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
	let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
	era * 146097 + doe - 719468
}

/// Parse `"2026-09-23T17:16:24+07:00"` (vsmov's wall-clock + offset) or the
/// classic `"2026-09-21T18:52:19.000Z"` → epoch millis. Both suffix shapes are
/// seen on live items, so the offset is honoured, not ignored.
fn parse_isodate_millis(input: &str) -> Option<i64> {
	let bytes = input.as_bytes();
	if bytes.len() < 19 {
		return None;
	}
	let num =
		|start: usize, len: usize| -> Option<i64> { input[start..start + len].parse::<i64>().ok() };
	let y = num(0, 4)?;
	let mo = num(5, 2)?;
	let d = num(8, 2)?;
	let h = num(11, 2)?;
	let mi = num(14, 2)?;
	let s = num(17, 2)?;

	let mut ms = 0_i64;
	let mut i = 19;
	if i < bytes.len() && bytes[i] == b'.' {
		let frac: String = input[i + 1..]
			.chars()
			.take_while(|c| c.is_ascii_digit())
			.collect();
		let mut normalized = frac.clone();
		if !normalized.is_empty() {
			while normalized.len() < 3 {
				normalized.push('0');
			}
			normalized.truncate(3);
			ms = normalized.parse().unwrap_or(0);
		}
		i += 1 + frac.len();
	}

	// Timezone suffix: "Z" (UTC) or "±HH:MM".
	let mut offset_s = 0_i64;
	if i < bytes.len() {
		match bytes[i] {
			b'Z' => {}
			b'+' | b'-' => {
				if i + 6 <= bytes.len() && bytes[i + 3] == b':' {
					let oh = input[i + 1..i + 3].parse::<i64>().ok()?;
					let om = input[i + 4..i + 6].parse::<i64>().ok()?;
					let total = oh * 3_600 + om * 60;
					offset_s = if bytes[i] == b'+' { total } else { -total };
				}
			}
			_ => {}
		}
	}

	let days = days_from_civil(y, mo, d);
	Some((days * 86_400 + h * 3_600 + mi * 60 + s - offset_s) * 1_000 + ms)
}

/// Strip HTML tags from `content` and decode a few common entities.
fn strip_html(input: &str) -> String {
	let mut result = String::with_capacity(input.len());
	let mut it = input.chars().peekable();
	while let Some(c) = it.next() {
		if c == '<' {
			for c2 in it.by_ref() {
				if c2 == '>' {
					break;
				}
			}
		} else {
			result.push(c);
		}
	}
	result
		.replace("&nbsp;", " ")
		.replace("&amp;", "&")
		.replace("&lt;", "<")
		.replace("&gt;", ">")
		.replace("&quot;", "\"")
		.replace("&#39;", "'")
		.replace("&hellip;", "...")
}

fn map_status(m: &VsmovMovie) -> AnimeStatus {
	match m.status.as_deref() {
		Some("completed") => AnimeStatus::Completed,
		Some("ongoing") => AnimeStatus::Ongoing,
		_ => AnimeStatus::Unknown,
	}
}

fn name_links(items: &[NameSlug]) -> Vec<CategoryLink> {
	items
		.iter()
		.filter(|c| !c.name.is_empty())
		.map(|c| CategoryLink {
			name: c.name.clone(),
			filters: Vec::new(),
		})
		.collect()
}

fn plain_links(items: &[String]) -> Vec<CategoryLink> {
	items
		.iter()
		.filter(|s| !s.is_empty())
		.map(|s| CategoryLink {
			name: s.clone(),
			filters: Vec::new(),
		})
		.collect()
}

fn year_link(year: Option<i32>) -> Option<CategoryLink> {
	year.map(|y| CategoryLink {
		name: format!("{y}"),
		filters: Vec::new(),
	})
}

/// Slug from the genre/country tables, falling back to a simple slugify.
fn genre_slug(name: &str) -> String {
	GENRES
		.iter()
		.find(|(n, _)| n.eq_ignore_ascii_case(name))
		.map(|(_, s)| s.to_string())
		.unwrap_or_else(|| slugify(name))
}

fn country_slug(name: &str) -> String {
	COUNTRIES
		.iter()
		.find(|(n, _)| n.eq_ignore_ascii_case(name))
		.map(|(_, s)| s.to_string())
		.unwrap_or_else(|| slugify(name))
}

/// Slugify fallback: keep ASCII chars, everything else → `-`.
fn slugify(name: &str) -> String {
	let mut slug = String::new();
	let mut prev_dash = false;
	for c in name.chars() {
		if c.is_ascii_alphanumeric() {
			slug.push(c.to_ascii_lowercase());
			prev_dash = false;
		} else if !prev_dash {
			slug.push('-');
			prev_dash = true;
		}
	}
	while slug.ends_with('-') {
		slug.pop();
	}
	slug
}

fn detect_stream_type(url: &str) -> StreamType {
	let lower = url.to_lowercase();
	if lower.contains(".m3u8") || lower.contains("/hls/") {
		StreamType::HLS
	} else if lower.contains(".mpd") {
		StreamType::DASH
	} else if lower.contains(".mp4") {
		StreamType::MP4
	} else {
		StreamType::OTHER
	}
}

/// Read the JSON string value of `key` inside a JSON object substring
/// (`{...,"key":"value",...}`).
fn read_json_string(s: &str, key: &str) -> Option<String> {
	let kidx = s.find(key)?;
	let after = &s[kidx + key.len()..];
	let colon = after.find(':')?;
	let val = &after[colon + 1..];
	let val = val.trim_start().strip_prefix('"')?;
	let mut result = String::new();
	let mut esc = false;
	for c in val.chars() {
		if esc {
			if c == 'n' {
				// "\n" escape — never appears in these urls, guard anyway.
				result.push('\n');
			} else {
				result.push(c);
			}
			esc = false;
		} else if c == '\\' {
			esc = true;
		} else if c == '"' {
			break;
		} else {
			result.push(c);
		}
	}
	Some(result)
}

/// The vsmov embed page advertises subtitle tracks in the inline
/// `playerOptions.subtitles` array — entries are flat
/// `{"name":...,"type":"local","url":"/video/<hash>/subtitle/vie_<ts>_<rand>.vtt",
/// "_inSubtitleFolder":true,"code":"vie"}`. Read each `(url, code)` pair with a
/// tiny JSON-object scanner — no full JSON parser needed for such a flat,
/// predictable structure.
fn extract_embed_subtitles(html: &str) -> Vec<(String, String)> {
	let Some(marker) = html.find("\"subtitles\"") else {
		return Vec::new();
	};
	let rest = &html[marker + "\"subtitles\"".len()..];
	let Some(colon) = rest.find(':') else {
		return Vec::new();
	};
	let Some(array) = rest[colon + 1..].trim_start().strip_prefix('[') else {
		return Vec::new();
	};
	let bytes = array.as_bytes();

	let mut out = Vec::new();
	let mut i = 0;
	while i < bytes.len() {
		while i < bytes.len()
			&& matches!(bytes[i], b' ' | b'\n' | b'\r' | b'\t' | b',')
		{
			i += 1;
		}
		if i >= bytes.len() || bytes[i] != b'{' {
			break;
		}
		// Find the closing brace of this object (no nested objects expected).
		let mut depth = 0_i32;
		let mut in_str = false;
		let mut esc = false;
		let mut j = i;
		while j < bytes.len() {
			let c = bytes[j];
			if in_str {
				if esc {
					esc = false;
				} else if c == b'\\' {
					esc = true;
				} else if c == b'"' {
					in_str = false;
				}
			} else {
				match c {
					b'"' => in_str = true,
					b'{' => depth += 1,
					b'}' => {
						depth -= 1;
						if depth == 0 {
							break;
						}
					}
					_ => {}
				}
			}
			j += 1;
		}
		if j >= bytes.len() {
			break;
		}
		let entry = &array[i..=j];
		if let Some(url) = read_json_string(entry, "\"url\"") {
			let code = read_json_string(entry, "\"code\"").unwrap_or_default();
			out.push((url, code));
		}
		i = j + 1;
	}
	out
}

/// Map the embed's subtitle code (`"vie"`/`"eng"`) to a media language tag.
fn subtitle_language(code: &str) -> String {
	match code {
		"vie" | "vi" => String::from("vi"),
		"eng" | "en" => String::from("en"),
		other if !other.is_empty() => String::from(other),
		_ => String::from("vi"),
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Fetch + build
// ────────────────────────────────────────────────────────────────────────────

/// Fetch movie detail (`{base}/phim/{slug}`) — root `movie` + root `episodes`.
fn fetch_detail(base: &str, slug: &str) -> Result<VsmovMovie> {
	let env: DetailEnvelope = Request::get(format!("{base}/phim/{slug}"))?.json_owned()?;
	let mut movie = env
		.movie
		.ok_or_else(|| error!("Không tìm thấy phim: {slug}"))?;
	if movie.episodes.is_empty() {
		movie.episodes = env.episodes.unwrap_or_default();
	}
	Ok(movie)
}

/// Fetch one list page and return (items, has_next_page).
fn fetch_page(url: &str) -> Result<(Vec<VsmovMovie>, bool)> {
	let env: ListEnvelope = Request::get(url)?.json_owned()?;
	let has_next = env
		.pagination
		.as_ref()
		.map(|pg| pg.current_page < pg.total_pages)
		.unwrap_or(false);
	let items = env.items.unwrap_or_default();
	let has_next = has_next || items.len() >= ITEMS_PER_PAGE;
	Ok((items, has_next))
}

/// Build a Lite card from a list item.
fn build_lite(m: &VsmovMovie, origin: &str) -> Anime {
	let poster = absolutize_opt(m.poster_url.as_deref().or(m.thumb_url.as_deref()), origin);
	Anime {
		key: m.slug.clone(),
		source_id: SOURCE_ID.into(),
		title: m.name.clone(),
		original_title: m.origin_name.clone(),
		cover: poster.clone().unwrap_or_default(),
		banner: poster,
		description: None,
		episode_count: parse_first_int(m.episode_total.as_deref()).unwrap_or(0),
		current_episode: m.episode_current.clone(),
		rating: None,
		rating_count: None,
		status: map_status(m),
		release_year: year_link(m.year),
		genres: name_links(&m.category),
		authors: plain_links(&m.director),
		studio: None,
		season_of: None,
		countries: name_links(&m.country),
		is_featured: false,
		views: m.view.unwrap_or(0),
		next_episode_air_info: None,
		quality_tag: m.quality.clone(),
		seasons: Vec::new(),
		episodes: None,
		url: Some(format!("{origin}/phim/{}", m.slug)),
	}
}

/// Full detail: `seasons` = playback servers (sorted by episode count desc).
fn build_full(m: &VsmovMovie, slug: &str, origin: &str) -> Anime {
	let mut groups: Vec<&EpisodeGroup> = m
		.episodes
		.iter()
		.filter(|g| !g.server_data.is_empty())
		.collect();
	groups.sort_by_key(|g| core::cmp::Reverse(g.server_data.len()));

	let seasons = groups
		.iter()
		.map(|g| {
			let name = normalize_server_name(&g.server_name);
			AnimeSeason {
				anime_id: format!("{slug}|{name}"),
				title: name.clone(),
				id: format!("{slug}|{name}"),
			}
		})
		.collect();

	let poster = absolutize_opt(m.poster_url.as_deref(), origin);
	let thumb = absolutize_opt(m.thumb_url.as_deref(), origin);
	let cover = poster.clone().or_else(|| thumb.clone());

	let episode_count = groups
		.first()
		.map(|g| g.server_data.len() as i32)
		.unwrap_or_else(|| parse_first_int(m.episode_total.as_deref()).unwrap_or(0));

	Anime {
		key: slug.to_string(),
		source_id: SOURCE_ID.into(),
		title: m.name.clone(),
		original_title: m.origin_name.clone(),
		cover: cover.unwrap_or_default(),
		banner: poster,
		description: Some(strip_html(m.content.as_deref().unwrap_or(""))),
		episode_count,
		current_episode: m.episode_current.clone(),
		rating: None,
		rating_count: None,
		status: map_status(m),
		release_year: year_link(m.year),
		genres: name_links(&m.category),
		authors: plain_links(&m.director),
		studio: None,
		season_of: None,
		countries: name_links(&m.country),
		is_featured: false,
		views: m.view.unwrap_or(0),
		next_episode_air_info: None,
		quality_tag: m.quality.clone(),
		seasons,
		episodes: None,
		url: Some(format!("{origin}/phim/{slug}")),
	}
}

/// Episodes for the selected server (None → first server = largest).
fn episodes_for_server(groups: &[EpisodeGroup], server: Option<&str>) -> Vec<Episode> {
	let group = match server {
		Some(name) => groups
			.iter()
			.find(|g| normalize_server_name(&g.server_name).eq_ignore_ascii_case(name))
			.or_else(|| groups.first()),
		None => groups.first(),
	};
	match group {
		Some(g) => g
			.server_data
			.iter()
			.map(|e| Episode {
				key: e.slug.clone(),
				episode_number: parse_episode_number(&e.name),
				title: Some(e.name.clone()),
				date_uploaded: None,
				thumbnail: None,
				quality: None,
				duration_seconds: None,
				url: None,
				language: Some(String::from("vi")),
				locked: false,
			})
			.collect(),
		None => Vec::new(),
	}
}

/// Stub episode for the Home screen ("Mới Cập Nhật") — episode count + date
/// from the list item.
fn home_episode(m: &VsmovMovie, origin: &str) -> Episode {
	let n = m.episode_current.as_deref().unwrap_or("");
	Episode {
		key: format!("{}__latest", m.slug),
		episode_number: parse_episode_number(n),
		title: Some(m.name.clone()),
		date_uploaded: m
			.modified
			.as_ref()
			.and_then(|md| md.time.as_deref())
			.and_then(parse_isodate_millis),
		thumbnail: absolutize_opt(m.thumb_url.as_deref().or(m.poster_url.as_deref()), origin),
		..Default::default()
	}
}

/// Map a listing id → slug on `/danh-sach/`.
fn listing_path(id: &str) -> &'static str {
	match id {
		"bo" => "phim-bo",
		"le" => "phim-le",
		"chieu-rap" => "phim-chieu-rap",
		"subteam" => "subteam",
		_ => "phim-moi-cap-nhat",
	}
}

/// Push filters into QueryParameters (genre/country name → slug).
fn apply_filters(qp: &mut QueryParameters, filters: &[FilterValue]) {
	for f in filters {
		match f {
			FilterValue::MultiSelect {
				id,
				included,
				excluded,
			} if id == "category" => {
				let slugs: Vec<String> = included
					.iter()
					.chain(excluded.iter())
					.map(|name| genre_slug(name))
					.collect();
				if !slugs.is_empty() {
					qp.push("category", Some(&slugs.join(",")));
				}
			}
			FilterValue::MultiSelect {
				id,
				included,
				excluded,
			} if id == "country" => {
				let slugs: Vec<String> = included
					.iter()
					.chain(excluded.iter())
					.map(|name| country_slug(name))
					.collect();
				if !slugs.is_empty() {
					qp.push("country", Some(&slugs.join(",")));
				}
			}
			FilterValue::Select { id, value } if id == "type" => {
				let t = match value.as_str() {
					"Phim bộ" => "series",
					"Phim lẻ" => "single",
					_ => "",
				};
				if !t.is_empty() {
					qp.push("type", Some(t));
				}
			}
			FilterValue::Range { id, from, to } if id == "year" => {
				if let (Some(a), Some(b)) = (from, to)
					&& (a - b).abs() < 0.5
					&& *a > 0.0
				{
					qp.push("year", Some(&format!("{}", *a as i32)));
				}
			}
			FilterValue::Sort {
				id,
				index,
				ascending,
			} if id == "sort" => {
				let field = match *index {
					1 => "year",
					2 => "name",
					_ => "modified.time",
				};
				qp.push("sort_field", Some(field));
				qp.push("sort_type", Some(if *ascending { "asc" } else { "desc" }));
			}
			_ => {}
		}
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Source
// ────────────────────────────────────────────────────────────────────────────

pub struct VsmovSource;

impl VsmovSource {
	fn base(&self) -> String {
		match defaults_get::<String>(SETTING_BASE_URL) {
			Some(s) if !s.trim().is_empty() => s.trim().to_string(),
			_ => String::from(DEFAULT_BASE),
		}
	}
}

impl Source for VsmovSource {
	fn new() -> Self {
		Self
	}

	fn get_search_anime_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<AnimePageResult> {
		let base = self.base();
		let origin = base_origin(&base);
		let mut qp = QueryParameters::with_capacity(6);
		let page_str = page.to_string();
		qp.push("page", Some(&page_str));
		if let Some(q) = query.as_ref()
			&& !q.is_empty()
		{
			qp.push("keyword", Some(q));
		}
		apply_filters(&mut qp, &filters);
		let qs = qp.to_string();
		// With a keyword → /tim-kiem; without one (browse filters/chips) → the
		// recently-updated list with the same filter set (the vsmov search
		// endpoint breaks on an empty `keyword`).
		let url = if qs.contains("keyword=") {
			format!("{base}/tim-kiem?{qs}")
		} else {
			format!("{base}/danh-sach/phim-moi-cap-nhat?{qs}")
		};
		let (items, has_next) = fetch_page(&url)?;
		Ok(AnimePageResult {
			entries: items.iter().map(|m| build_lite(m, &origin)).collect(),
			has_next_page: has_next,
		})
	}

	fn get_anime_update(
		&self,
		mut anime: Anime,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Anime> {
		let (slug, server) = split_server(&anime.key);
		let server = server.map(|s| s.to_string());
		if needs_details || needs_chapters {
			let base = self.base();
			let movie = fetch_detail(&base, slug)?;
			if needs_details {
				let full = build_full(&movie, slug, &base_origin(&base));
				anime.copy_from(full);
			}
			if needs_chapters {
				anime.episodes = Some(episodes_for_server(&movie.episodes, server.as_deref()));
			}
		}
		Ok(anime)
	}

	fn get_stream_list(&self, anime: Anime, _episode: Episode) -> Result<Vec<StreamInfo>> {
		let quality = anime.quality_tag.clone().unwrap_or_default();
		Ok(anime
			.seasons
			.iter()
			.map(|s| {
				// Prefer the server from anime_id ("{slug}|{server}"), fallback to title.
				let server = s
					.anime_id
					.split_once('|')
					.map(|(_, name)| name)
					.unwrap_or(&s.title);
				StreamInfo {
					key: server.to_string(),
					name: server.to_string(),
					quality: quality.clone(),
				}
			})
			.collect())
	}

	fn get_stream(&self, anime: Anime, episode: Episode, stream: StreamInfo) -> Result<StreamData> {
		let base = self.base();
		let (slug, _) = split_server(&anime.key);
		let movie = fetch_detail(&base, slug)?;
		let origin = base_origin(&base);

		// Look in the requested server first; if that server lacks the episode
		// (e.g. the app auto-resolves the first server for an episode that only
		// exists on another one) fall back to every group.
		let entry = movie
			.episodes
			.iter()
			.find(|g| normalize_server_name(&g.server_name).eq_ignore_ascii_case(&stream.key))
			.and_then(|g| g.server_data.iter().find(|e| e.slug == episode.key))
			.or_else(|| {
				movie
					.episodes
					.iter()
					.flat_map(|g| g.server_data.iter())
					.find(|e| e.slug == episode.key)
			});

		let Some(entry) = entry else {
			bail!(
				"Nguồn phát {} không có tập {} ({}).",
				stream.key,
				episode.episode_number,
				episode.key
			);
		};

		let Some(video_url) = entry.link_embed.as_deref() else {
			bail!(
				"Không tìm thấy liên kết phát cho tập {}.",
				episode.episode_number
			);
		};

		// The embed page (https://vX.streamvsmov.com/video/<hash>) → the master
		// playlist the page itself plays (…/stream/<hash>/master.m3u8).
		let Some(url) = embed_to_m3u8(video_url) else {
			bail!("Liên kết phát không hợp lệ: {video_url}");
		};

		let mut headers = komorei::HashMap::new();
		headers.insert(String::from("Referer"), format!("{origin}/"));

		// External `.vtt` subtitles live on the embed page's `playerOptions`.
		// Best-effort: a failed embed fetch must never break playback.
		let mut subtitles: Vec<SubtitleInfo> = Vec::new();
		if let Some(html) = Request::get(video_url).ok().and_then(|r| r.string().ok()) {
			let host = embed_host(video_url);
			for (path, code) in extract_embed_subtitles(&html) {
				let url = absolutize_url(&path, host.as_deref().unwrap_or(&origin));
				subtitles.push(SubtitleInfo {
					url,
					language: subtitle_language(&code),
					label: Some(if code.is_empty() {
						String::from("Phụ đề")
					} else {
						code.clone()
					}),
					headers: komorei::HashMap::new(),
				});
			}
		}

		let stream_type = detect_stream_type(&url);

		Ok(StreamData {
			url,
			stream_type,
			is_content: false,
			headers,
			subtitles,
			intro: None,
			outro: None,
		})
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Listings
// ────────────────────────────────────────────────────────────────────────────

impl ListingProvider for VsmovSource {
	fn get_anime_list(&self, listing: Listing, page: i32) -> Result<AnimePageResult> {
		if !matches!(listing.kind, ListingKind::List) {
			return Ok(AnimePageResult::default());
		}
		let base = self.base();
		let origin = base_origin(&base);
		let path = listing_path(&listing.id);
		let (items, has_next) = fetch_page(&format!("{base}/danh-sach/{path}?page={page}"))?;
		Ok(AnimePageResult {
			entries: items.iter().map(|m| build_lite(m, &origin)).collect(),
			has_next_page: has_next,
		})
	}
}

impl DynamicListings for VsmovSource {
	fn get_dynamic_listings(&self) -> Result<Vec<Listing>> {
		Ok(vec![
			Listing {
				id: String::from("latest"),
				name: String::from("Mới Cập Nhật"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("bo"),
				name: String::from("Phim Bộ"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("le"),
				name: String::from("Phim Lẻ"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("chieu-rap"),
				name: String::from("Phim Chiếu Rạp"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("subteam"),
				name: String::from("Subteam"),
				kind: ListingKind::List,
			},
		])
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Home
// ────────────────────────────────────────────────────────────────────────────

impl Home for VsmovSource {
	fn get_home(&self) -> Result<HomeLayout> {
		let base = self.base();
		let origin = base_origin(&base);
		let urls = [
			format!("{base}/danh-sach/phim-moi-cap-nhat?page=1"),
			format!("{base}/danh-sach/phim-bo?page=1"),
			format!("{base}/danh-sach/phim-le?page=1"),
		];

		// 3 parallel requests for the home rows.
		let mut requests = Vec::with_capacity(3);
		for u in &urls {
			requests.push(Request::get(u.as_str())?);
		}
		let responses = Request::send_all(requests);

		let mut latest: Vec<VsmovMovie> = Vec::new();
		let mut bo: Vec<VsmovMovie> = Vec::new();
		let mut le: Vec<VsmovMovie> = Vec::new();
		for (i, resp) in responses.into_iter().enumerate() {
			let Ok(resp) = resp else { continue };
			let Ok(env) = resp.get_json_owned::<ListEnvelope>() else {
				continue;
			};
			let items = env.items.unwrap_or_default();
			match i {
				0 => latest = items,
				1 => bo = items,
				_ => le = items,
			}
		}

		let mut components = Vec::new();

		// Featured (banner)
		if !latest.is_empty() {
			let links: Vec<Link> = latest
				.iter()
				.take(5)
				.map(|m| Link {
					title: m.name.clone(),
					image_url: absolutize_opt(
						m.poster_url.as_deref().or(m.thumb_url.as_deref()),
						&origin,
					),
					value: Some(LinkValue::Anime(build_lite(m, &origin))),
					..Default::default()
				})
				.collect();
			components.push(HomeComponent {
				title: Some(String::from("Nổi Bật")),
				value: HomeComponentValue::ImageScroller {
					links,
					auto_scroll_interval: Some(5.0),
					width: Some(800),
					height: Some(450),
				},
				..Default::default()
			});

			// Recently updated (real episode count + date)
			let listed: Vec<AnimeWithEpisode> = latest
				.iter()
				.take(8)
				.map(|m| AnimeWithEpisode {
					anime: build_lite(m, &origin),
					episode: home_episode(m, &origin),
				})
				.collect();
			components.push(HomeComponent {
				title: Some(String::from("Mới Cập Nhật")),
				value: HomeComponentValue::AnimeEpisodeList {
					page_size: Some(4),
					entries: listed,
					listing: Some(Listing {
						id: String::from("latest"),
						name: String::from("Mới Cập Nhật"),
						kind: ListingKind::List,
					}),
				},
				..Default::default()
			});
		}

		// Series (site category "Phim Bộ")
		if !bo.is_empty() {
			let entries: Vec<Link> = bo
				.iter()
				.take(10)
				.map(|m| Link {
					title: m.name.clone(),
					subtitle: m.episode_current.clone(),
					image_url: absolutize_opt(
						m.poster_url.as_deref().or(m.thumb_url.as_deref()),
						&origin,
					),
					value: Some(LinkValue::Anime(build_lite(m, &origin))),
				})
				.collect();
			components.push(HomeComponent {
				title: Some(String::from("Phim Bộ")),
				value: HomeComponentValue::Scroller {
					entries,
					listing: Some(Listing {
						id: String::from("bo"),
						name: String::from("Phim Bộ"),
						kind: ListingKind::List,
					}),
				},
				..Default::default()
			});
		}

		// Single movies (site category "Phim Lẻ")
		if !le.is_empty() {
			let entries: Vec<Link> = le
				.iter()
				.take(8)
				.map(|m| Link {
					title: m.name.clone(),
					subtitle: m.episode_current.clone(),
					image_url: absolutize_opt(
						m.poster_url.as_deref().or(m.thumb_url.as_deref()),
						&origin,
					),
					value: Some(LinkValue::Anime(build_lite(m, &origin))),
				})
				.collect();
			components.push(HomeComponent {
				title: Some(String::from("Phim Lẻ")),
				value: HomeComponentValue::AnimeList {
					ranking: false, /* list is ordered by update, not popularity */
					page_size: Some(4),
					entries,
					listing: Some(Listing {
						id: String::from("le"),
						name: String::from("Phim Lẻ"),
						kind: ListingKind::List,
					}),
				},
				..Default::default()
			});
		}

		// Genre chips (site category "Thể Loại") → search with the "category" filter
		let filters: Vec<FilterItem> = GENRES
			.iter()
			.map(|(name, _)| FilterItem {
				title: name.to_string(),
				values: Some(vec![FilterValue::MultiSelect {
					id: String::from("category"),
					included: vec![name.to_string()],
					excluded: Vec::new(),
				}]),
			})
			.collect();
		components.push(HomeComponent {
			title: Some(String::from("Thể Loại")),
			value: HomeComponentValue::Filters(filters),
			..Default::default()
		});

		// Quick links to the listings
		let links: Vec<Link> = [
			("latest", "Mới Cập Nhật"),
			("bo", "Phim Bộ"),
			("le", "Phim Lẻ"),
			("chieu-rap", "Phim Chiếu Rạp"),
			("subteam", "Subteam"),
		]
		.iter()
		.map(|(id, name)| Link {
			title: name.to_string(),
			value: Some(LinkValue::Listing(Listing {
				id: id.to_string(),
				name: name.to_string(),
				kind: ListingKind::List,
			})),
			..Default::default()
		})
		.collect();
		components.push(HomeComponent {
			title: Some(String::from("Danh Sách")),
			value: HomeComponentValue::Links(links),
			..Default::default()
		});

		Ok(HomeLayout { components })
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Filters
// ────────────────────────────────────────────────────────────────────────────

impl DynamicFilters for VsmovSource {
	fn get_dynamic_filters(&self) -> Result<Vec<Filter>> {
		Ok(vec![
			TextFilter {
				id: "search".into(),
				title: Some("Tìm kiếm".into()),
				placeholder: Some("Tên phim...".into()),
				..Default::default()
			}
			.into(),
			SortFilter {
				id: "sort".into(),
				title: Some("Sắp xếp".into()),
				can_ascend: true,
				default: Some(SortFilterDefault {
					index: 0,
					ascending: false,
				}),
				options: vec![
					"Mới cập nhật".into(),
					"Năm phát hành".into(),
					"Tên A-Z".into(),
				],
				..Default::default()
			}
			.into(),
			MultiSelectFilter {
				id: "category".into(),
				title: Some("Thể loại".into()),
				is_genre: true,
				uses_tag_style: true,
				options: GENRES.iter().map(|(n, _)| Cow::Borrowed(*n)).collect(),
				..Default::default()
			}
			.into(),
			MultiSelectFilter {
				id: "country".into(),
				title: Some("Quốc gia".into()),
				is_genre: false,
				uses_tag_style: true,
				options: COUNTRIES.iter().map(|(n, _)| Cow::Borrowed(*n)).collect(),
				..Default::default()
			}
			.into(),
			SelectFilter {
				id: "type".into(),
				title: Some("Loại phim".into()),
				options: vec!["Tất cả".into(), "Phim bộ".into(), "Phim lẻ".into()],
				..Default::default()
			}
			.into(),
			RangeFilter {
				id: "year".into(),
				title: Some("Năm phát hành".into()),
				min: Some(1980.0),
				max: Some(2027.0),
				decimal: false,
				..Default::default()
			}
			.into(),
			Filter::note(
				"Nội dung khai thác từ API VSMov (https://vsmov.com). Có thể đổi base URL ở Cài đặt nguồn.",
			),
		])
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Settings
// ────────────────────────────────────────────────────────────────────────────

impl DynamicSettings for VsmovSource {
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		Ok(vec![
			TextSetting {
				key: SETTING_BASE_URL.into(),
				title: "Địa chỉ API VSMov".into(),
				placeholder: Some("https://vsmov.com/api".into()),
				notification: Some("base_url_changed".into()),
				refreshes: Some(vec!["content".into(), "listings".into()]),
				default: Some(DEFAULT_BASE.into()),
				..Default::default()
			}
			.into(),
			ButtonSetting {
				key: "clear_cache".into(),
				title: "Xoá bộ nhớ nguồn".into(),
				notification: Some("clear_cache".into()),
				..Default::default()
			}
			.into(),
		])
	}
}

impl NotificationHandler for VsmovSource {
	fn handle_notification(&self, notification: String) {
		// The source has no meaningful cache; just record the change (base_url
		// is read directly via defaults_get on every request).
		defaults_set(
			SETTING_LAST_NOTIFICATION,
			DefaultValue::String(notification),
		);
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Deep links
// ────────────────────────────────────────────────────────────────────────────

impl DeepLinkHandler for VsmovSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		// vsmov plays from a modal on the movie page — there is no `xem-phim`
		// route to deep-link episodes.
		if let Some(idx) = url.find("/phim/") {
			let slug = &url[idx + "/phim/".len()..];
			let slug = slug.trim_end_matches('/');
			if !slug.is_empty() {
				return Ok(Some(DeepLinkResult::Anime {
					key: slug.to_string(),
				}));
			}
		}
		Ok(None)
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Migration — vsmov does not change ids across versions, keep identity.
// ────────────────────────────────────────────────────────────────────────────

impl MigrationHandler for VsmovSource {
	fn handle_anime_migration(&self, key: String) -> Result<String> {
		Ok(key)
	}

	fn handle_episode_migration(&self, _anime_key: String, episode_key: String) -> Result<String> {
		Ok(episode_key)
	}
}

register_source!(
	VsmovSource,
	ListingProvider,
	Home,
	DynamicFilters,
	DynamicSettings,
	DynamicListings,
	NotificationHandler,
	DeepLinkHandler,
	MigrationHandler
);

// ────────────────────────────────────────────────────────────────────────────
// Tests (pure lib — run on the host with `cargo test`, no wasm needed)
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
	use super::*;
	use komorei_test::komorei_test;

	#[komorei_test]
	fn parses_isodate_millis_with_offset_and_z() {
		// vsmov wall-clock timezone (+07:00) → correct UTC epoch.
		assert_eq!(
			parse_isodate_millis("2026-09-23T17:16:24+07:00"),
			Some(1_790_158_584_000)
		);
		assert_eq!(
			parse_isodate_millis("2026-09-21T00:19:37+07:00"),
			Some(1_789_924_777_000)
		);
		// classic UTC form also appears in some fields.
		assert_eq!(
			parse_isodate_millis("2026-09-21T18:52:19.000Z"),
			Some(1_790_016_739_000)
		);
		// negative offset
		assert_eq!(
			parse_isodate_millis("2026-09-21T00:00:00-05:00"),
			Some(1_789_966_800_000)
		);
		assert_eq!(parse_isodate_millis("2026-09-21"), None);
		assert_eq!(parse_isodate_millis("not-a-date"), None);
	}

	#[komorei_test]
	fn protects_against_aliasing_timestamps() {
		// 2026-09-23T17:16:24+07:00 == 2026-09-23T10:16:24Z (7 h earlier).
		assert_eq!(
			parse_isodate_millis("2026-09-23T17:16:24+07:00"),
			parse_isodate_millis("2026-09-23T10:16:24Z")
		);
	}

	#[komorei_test]
	fn splits_season_key_into_slug_and_server() {
		assert_eq!(
			split_server("nhat-au-xuan|Vietsub"),
			("nhat-au-xuan", Some("Vietsub"))
		);
		assert_eq!(split_server("nhat-au-xuan"), ("nhat-au-xuan", None));
	}

	#[komorei_test]
	fn resolves_embed_to_master_playlist() {
		assert_eq!(
			embed_to_m3u8("https://v9.streamvsmov.com/video/6a002aba-05c7-4ce3-a89a-0b214eac60f8"),
			Some("https://v9.streamvsmov.com/stream/6a002aba-05c7-4ce3-a89a-0b214eac60f8/master.m3u8".into())
		);
		// already a playlist → pass through
		assert_eq!(
			embed_to_m3u8("https://v9.streamvsmov.com/stream/x/master.m3u8"),
			Some("https://v9.streamvsmov.com/stream/x/master.m3u8".into())
		);
		assert_eq!(embed_to_m3u8("https://v9.streamvsmov.com/video/"), None);
		assert_eq!(embed_to_m3u8("https://example.com/"), None);
		assert_eq!(embed_to_m3u8("not-a-url"), None);
	}

	#[komorei_test]
	fn embed_host_extracts_origin() {
		assert_eq!(
			embed_host("https://v9.streamvsmov.com/video/abc"),
			Some("https://v9.streamvsmov.com".into())
		);
		assert_eq!(embed_host("not-a-url"), None);
	}

	#[komorei_test]
	fn reads_subtitle_tracks_from_player_options() {
		let html = r##"<!doctype html>
<html><head></head><body>
<script>
  const playerOptions = {
    "qualitys": [],
    "links": [],
    "subtitles": [
      {"name":"vie 1789619837502 zzigvu","type":"local","url":"/video/6a002aba-05c7-4ce3-a89a-0b214eac60f8/subtitle/vie_1789619837502_zzigvu.vtt","_inSubtitleFolder":true,"code":"vie"},
      {"name":"eng 1789619837451 zzigvu","type":"local","url":"/video/6a002aba-05c7-4ce3-a89a-0b214eac60f8/subtitle/eng_1789619837451_zzigvu.vtt","_inSubtitleFolder":true,"code":"eng"}
    ],
    "thumb": "..."
  };
</script>
</body></html>"##;
		let subs = extract_embed_subtitles(html);
		assert_eq!(subs.len(), 2);
		assert_eq!(subs[0].1, "vie");
		assert!(subs[0].0.contains("/subtitle/vie_1789619837502_zzigvu.vtt"));
		assert_eq!(subs[1].1, "eng");
	}

	#[komorei_test]
	fn missing_player_options_yield_no_subtitles() {
		assert!(extract_embed_subtitles("<html>no player here</html>").is_empty());
		assert!(extract_embed_subtitles("").is_empty());
	}

	#[komorei_test]
	fn maps_subtitle_codes_to_language_tags() {
		assert_eq!(subtitle_language("vie"), "vi");
		assert_eq!(subtitle_language("vi"), "vi");
		assert_eq!(subtitle_language("eng"), "en");
		assert_eq!(subtitle_language("tha"), "tha");
		assert_eq!(subtitle_language(""), "vi");
	}

	#[komorei_test]
	fn decodes_empty_object_poster_as_none() {
		#[derive(Deserialize)]
		struct Msg {
			#[serde(default, deserialize_with = "de_opt_pic")]
			poster_url: Option<String>,
		}
		// normal URL
		let m: Msg = serde_json::from_str(r#"{"poster_url":"https://vsmov.com/s.jpg"}"#).unwrap();
		assert_eq!(m.poster_url.as_deref(), Some("https://vsmov.com/s.jpg"));
		// empty object (films without a poster)
		let m: Msg = serde_json::from_str(r#"{"poster_url":{}}"#).unwrap();
		assert_eq!(m.poster_url, None);
		// null
		let m: Msg = serde_json::from_str(r#"{"poster_url":null}"#).unwrap();
		assert_eq!(m.poster_url, None);
		// absent
		let m: Msg = serde_json::from_str(r#"{}"#).unwrap();
		assert_eq!(m.poster_url, None);
	}

	#[komorei_test]
	fn parses_leading_digits_of_episode_labels() {
		assert_eq!(parse_episode_number("1"), "1");
		assert_eq!(parse_episode_number("Tập 16"), "16");
		assert_eq!(parse_episode_number("Full"), "1");
		assert_eq!(parse_first_int(Some("30")), Some(30));
		assert_eq!(parse_first_int(Some("Full")), None);
	}

	#[komorei_test]
	fn absolutizes_relative_and_protocol_relative_images() {
		let origin = "https://vsmov.com";
		assert_eq!(
			absolutize_url("//vsmov.com/storage/a.jpg", origin),
			"https://vsmov.com/storage/a.jpg"
		);
		assert_eq!(
			absolutize_url("/storage/b.jpg", origin),
			"https://vsmov.com/storage/b.jpg"
		);
		assert_eq!(
			absolutize_url("https://vsmov.com/storage/c.jpg", origin),
			"https://vsmov.com/storage/c.jpg"
		);
		assert_eq!(absolutize_opt(None, origin), None);
		// base with a /api suffix still yields the site origin
		assert_eq!(base_origin("https://vsmov.com/api"), "https://vsmov.com");
	}

	#[komorei_test]
	fn detects_stream_type_from_url() {
		assert_eq!(
			detect_stream_type("https://v9.streamvsmov.com/stream/x/master.m3u8"),
			StreamType::HLS
		);
		assert_eq!(detect_stream_type("https://cdn.test/a.mp4"), StreamType::MP4);
		assert_eq!(detect_stream_type("https://cdn.test/a.mpd"), StreamType::DASH);
		assert_eq!(
			detect_stream_type("https://v9.streamvsmov.com/video/abc"),
			StreamType::OTHER
		);
	}

	#[komorei_test]
	fn strips_html_tags_and_entities_from_content() {
		assert_eq!(
			strip_html("<p>Người đàn ông quyết đoán&hellip;</p>&nbsp;"),
			"Người đàn ông quyết đoán... "
		);
		assert_eq!(strip_html("plain text"), "plain text");
	}

	#[komorei_test]
	fn maps_genre_and_country_names_to_vsmov_slugs() {
		assert_eq!(genre_slug("Hành Động"), "hanh-dong");
		assert_eq!(genre_slug("Kinh Dị"), "kinh-di");
		assert_eq!(genre_slug("Sci-Fi"), "sci-fi"); // not in the table → ASCII slugify
		assert_eq!(country_slug("Mỹ"), "my");
		assert_eq!(country_slug("Trung Quốc"), "trung-quoc");
	}

	#[komorei_test]
	fn maps_vsmov_status_enums() {
		let completed = VsmovMovie {
			status: Some(String::from("completed")),
			..Default::default()
		};
		let ongoing = VsmovMovie {
			status: Some(String::from("ongoing")),
			..Default::default()
		};
		assert_eq!(map_status(&completed), AnimeStatus::Completed);
		assert_eq!(map_status(&ongoing), AnimeStatus::Ongoing);
		assert_eq!(map_status(&VsmovMovie::default()), AnimeStatus::Unknown);
	}

	#[komorei_test]
	fn season_key_roundtrip_builds_linked_anime_ids() {
		// fake detail JSON: the main server first (like the real vsmov)
		let groups = vec![
			EpisodeGroup {
				server_name: String::from("Vietsub\r\n #1"), // site sends trailing noise
				server_data: vec![
					EpisodeEntry {
						slug: String::from("tap-1"),
						..Default::default()
					},
					EpisodeEntry {
						slug: String::from("tap-2"),
						..Default::default()
					},
				],
			},
			EpisodeGroup {
				server_name: String::from("Trailer"),
				server_data: vec![EpisodeEntry {
					slug: String::from("trailer-1"),
					..Default::default()
				}],
			},
		];
		let full = build_full(
			&VsmovMovie {
				name: String::from("Nhất Âu Xuân"),
				episodes: groups.clone(),
				..Default::default()
			},
			"nhat-au-xuan",
			"https://vsmov.com",
		);
		// the largest server is sorted first, names whitespace-normalised
		assert_eq!(
			full.seasons
				.iter()
				.map(|s| s.anime_id.as_str())
				.collect::<Vec<_>>(),
			vec!["nhat-au-xuan|Vietsub #1", "nhat-au-xuan|Trailer"]
		);
		// the season key returns the episodes of that server
		assert_eq!(
			episodes_for_server(&groups, Some("Vietsub #1"))
				.iter()
				.map(|e| e.key.as_str())
				.collect::<Vec<_>>(),
			vec!["tap-1", "tap-2"]
		);
		assert_eq!(
			episodes_for_server(&groups, Some("Trailer"))
				.iter()
				.map(|e| e.key.as_str())
				.collect::<Vec<_>>(),
			vec!["trailer-1"]
		);
		// default (no server selected) → first server = largest
		assert_eq!(
			episodes_for_server(&groups, None)
				.iter()
				.map(|e| e.key.as_str())
				.collect::<Vec<_>>(),
			vec!["tap-1", "tap-2"]
		);
	}

	#[komorei_test]
	fn reads_root_episodes_from_vsmov_detail() {
		// `/api/phim/{slug}` → `{ status, msg, movie, episodes }` at the ROOT.
		// (`@@` is swapped for a real `\r\n` — raw strings can't carry one.)
		let json = r##"{
			"status": true,
			"msg": "done",
			"movie": { "name": "Nhất Âu Xuân", "slug": "nhat-au-xuan", "poster_url": {} },
			"episodes": [
				{ "server_name": "Vietsub@@ #1", "server_data": [
					{ "name": "1", "slug": "tap-1", "filename": "1",
					  "link_embed": "https://v9.streamvsmov.com/video/6a002aba-05c7-4ce3-a89a-0b214eac60f8" }
				] }
			]
		}"##
		.replace("@@", "\\r\\n");
		let env: DetailEnvelope = serde_json::from_str(&json).expect("detail envelope decodes");
		assert!(env.movie.is_some());
		assert_eq!(env.movie.as_ref().unwrap().poster_url, None); // `{}` → None
		let root_eps = env.episodes.clone().unwrap_or_default();
		assert_eq!(root_eps.len(), 1);
		assert_eq!(root_eps[0].server_data[0].slug, "tap-1");
		assert_eq!(
			root_eps[0].server_data[0].link_embed.as_deref(),
			Some("https://v9.streamvsmov.com/video/6a002aba-05c7-4ce3-a89a-0b214eac60f8")
		);
		// fetch_detail merge: root-level episodes win when the movie has none.
		let mut env = env;
		let movie = VsmovMovie { episodes: Vec::new(), ..Default::default() };
		env.movie = Some(movie);
		let mut full = env.movie.take().unwrap();
		if full.episodes.is_empty() {
			full.episodes = env.episodes.unwrap_or_default();
		}
		assert_eq!(full.episodes.len(), 1);
		assert_eq!(
			normalize_server_name(&full.episodes[0].server_name),
			"Vietsub #1"
		);
	}

	#[komorei_test]
	fn reads_root_pagination_from_vsmov_list() {
		let json = r##"{
			"status": true,
			"msg": "done",
			"items": [
				{ "name": "A", "slug": "a", "poster_url": "https://vsmov.com/a.jpg", "year": 2024 }
			],
			"pagination": { "totalItems": 19813, "totalItemsPerPage": 24, "currentPage": 1, "totalPages": 826 }
		}"##;
		let env: ListEnvelope = serde_json::from_str(json).expect("list envelope decodes");
		let has_next = env
			.pagination
			.as_ref()
			.map(|pg| pg.current_page < pg.total_pages)
			.unwrap_or(false);
		assert!(has_next);
		let items = env.items.unwrap_or_default();
		assert_eq!(items.len(), 1);
		assert_eq!(items[0].slug, "a");
	}

	#[komorei_test]
	fn reads_catalog_items_under_data() {
		// `/api/the-loai` / `/api/quoc-gia` keep the classic envelope.
		let json = r##"{
			"status": "success",
			"message": "",
			"data": { "items": [
				{ "_id": 85, "name": "Action & Adventure", "slug": "action-adventure" },
				{ "_id": 17, "name": "Bí Ẩn", "slug": "bi-an" }
			] }
		}"##;
		let env: CatalogEnvelope = serde_json::from_str(json).expect("catalog envelope decodes");
		let items = env.data.map(|d| d.items).unwrap_or_default();
		assert_eq!(items.len(), 2);
		assert_eq!(items[0].slug, "action-adventure");
	}

	#[komorei_test]
	fn search_item_without_episode_metadata_decodes() {
		// list/search items are lite — no episodes, no quality, integer `_id`.
		let json = r##"{
			"_id": 52951, "imdb": { "id": null }, "tmdb": { "type": "tv", "id": "287994" },
			"modified": { "time": "2026-09-21T00:19:37+07:00" },
			"name": "Pháp Y Tần Minh", "origin_name": "Once Upon a Time",
			"slug": "phap-y-tan-minh", "poster_url": {}, "thumb_url": "https://vsmov.com/t.jpg",
			"year": 2026
		}"##;
		let m: VsmovMovie = serde_json::from_str(json).expect("movie item decodes");
		assert_eq!(m.slug, "phap-y-tan-minh");
		assert_eq!(m.poster_url, None);
		assert_eq!(m.episode_total, None);
		assert_eq!(m.quality, None);
		let origin = "https://vsmov.com";
		let lite = build_lite(&m, origin);
		assert_eq!(lite.key, "phap-y-tan-minh");
		assert_eq!(lite.banner.as_deref(), Some("https://vsmov.com/t.jpg"));
		assert_eq!(lite.release_year.as_ref().map(|c| c.name.as_str()), Some("2026"));
	}
}