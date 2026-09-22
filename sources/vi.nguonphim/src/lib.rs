//! # Nguồn Phim source (`vi.nguonphim`)
//!
//! Scrapes the **Nguồn Phim** REST API at `https://api.nguonphim.net` — the
//! "Nguồn" movie network that also runs `phim.nguonc.com` (Nguồn C) and the
//! OnTV fronts (`ngontv.com`, `nguonphim.net`, `nguonphime.site`). The API is
//! served from the `api.*` vhosts of that box — mirrors:
//! `api.ngontv.com`, `api.nguonphime.site` (swap via the `base_url` setting).
//! The web fronts bounce through an "NP Checker" interstitial
//! (`nguonphime.site/site/site/embed/?url=…`, sets a PHP session then JS
//! forwards), but the `api.*` hosts answer directly. Note: the API was under
//! maintenance ("dừng hoạt động để nâng cấp") when this source was written —
//! the endpoint shapes below follow the network's nguonc-style engine
//! (`/api-document` on `phim.nguonc.com`) and are exercised in-wasm offline:
//!
//! - lists: `GET /api/films/phim-moi-cap-nhat?page=N` (latest),
//!   `GET /api/films/danh-sach/{slug}?page=N` (phim-bo/phim-le/tv-shows/
//!   dang-chieu), `GET /api/films/the-loai/{slug}?page=N` (genre),
//!   `GET /api/films/quoc-gia/{slug}?page=N` (country),
//!   `GET /api/films/nam-phat-hanh/{year}?page=N` (year)
//! - search: `GET /api/films/search?keyword={q}&page=N`
//! - detail: `GET /api/film/{slug}` → `{ status, movie }` where
//!   `movie.episodes[] = { server_name, items[] = { name, slug, embed } }`
//!
//! Every envelope is `{ status, paginate: {current_page,total_page,..},
//! items[] }` (detail: `{ status, movie }`).
//!
//! ## Streams — signed HLS behind a two-step grant
//!
//! The Nguồn network uses the same player infrastructure as Nguồn C: each
//! episode is an **embed iframe** (`https://embed{N}.streamc.xyz/embed.php?
//! hash=<hex>`) whose player (JWPlayer + Cloudflare Turnstile "verification"
//! layer) resolves the real playlist. `get_stream` replays the player's own
//! two POSTs against the embed endpoint:
//!
//! 1. `POST embed.php?hash=…` with `{ action:"bootstrap", referrer,
//!    frame_origins, request_grant: true, playlist_format:"hls",
//!    bootstrap_format:"json" }` → `{ bootstrap: <signed JWT>, nonce,
//!    turnstileEnabled, … }`.
//! 2. `POST` it again with `{ action:"issue", bootstrap: <JWT>,
//!    turnstile_response:"", playlist_format:"hls", … }` → `{ playlist:
//!    <signed url>, playlistFormat:"hls", issuedAt, expiresAt }`.
//!
//! The granted playlist is a **plain HLS master/media playlist** (this source
//! always requests the `"hls"` format so no AES-GCM unwrapping is needed);
//! its segment URLs are disguised with a `.png` extension but carry real
//! MPEG-TS bytes. The segment CDN rejects requests without a `Referer`, so the
//! resolved `StreamData` carries `Referer: {embed origin}/` for every
//! sub-request (playlist + segments). The signed playlist expires after ~4h.
//!
//! Embeds whose bootstrap reports `turnstileEnabled: true` cannot be solved
//! from a plain HTTP source (Cloudflare Turnstile needs an interactive
//! browser) — the `issue` call fails and the source surfaces the server error.
//!
//! ## Data model mapping to Komorei (mirrors `vi.ophim` / `vi.nguonc`)
//!
//! - **Season = one playback server.** `AnimeSeason.anime_id` is encoded as
//!   `"{slug}|{server_name}"`; `get_anime_update({anime}, needs_chapters)`
//!   strips the part after `|` and returns that server's episodes. Servers
//!   are sorted by episode count descending.
//! - **Episode key = `slug` of the entry** (e.g. `tap-1`).
//! - `get_stream_list`: each season is a `StreamInfo` (key = server name).
//!   `get_stream`: refetches the detail, finds the embed by server + episode,
//!   then walks the bootstrap → issue grant flow. If the selected server lacks
//!   that episode it falls back to the first group containing it.

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
	MigrationHandler, MultiSelectFilter, NotificationHandler, Result, SelectFilter, Setting,
	SortFilter, SortFilterDefault, Source, StreamData, StreamInfo, StreamType, TextFilter,
	TextSetting,
	helpers::uri::encode_uri_component,
	imports::defaults::{DefaultValue, defaults_get, defaults_set},
	imports::net::Request,
	prelude::*,
	serde::Deserialize,
};

const SOURCE_ID: &str = "vi.nguonphim";
const DEFAULT_BASE: &str = "https://api.nguonphim.net";
const SETTING_BASE_URL: &str = "base_url";
const SETTING_LAST_NOTIFICATION: &str = "last_notification";

/// The stream grant always requests the plain unencrypted HLS format so the
/// app can play the result directly (no JS AES-GCM unwrapping in the player).
const PLAYLIST_FORMAT: &str = "hls";

// ────────────────────────────────────────────────────────────────────────────
// Catalogs (name → API slug). Slugs were taken from the site's own nav menu
// (`/the-loai/…`, `/quoc-gia/…`) and verified against the live API.
// ────────────────────────────────────────────────────────────────────────────

const GENRES: &[(&str, &str)] = &[
	("Hành Động", "hanh-dong"),
	("Phiêu Lưu", "phieu-luu"),
	("Hoạt Hình", "hoat-hinh"),
	("Hài", "hai"),
	("Hình Sự", "hinh-su"),
	("Tài Liệu", "tai-lieu"),
	("Chính Kịch", "chinh-kich"),
	("Gia Đình", "gia-dinh"),
	("Giả Tưởng", "gia-tuong"),
	("Lịch Sử", "lich-su"),
	("Kinh Dị", "kinh-di"),
	("Nhạc", "nhac"),
	("Bí Ẩn", "bi-an"),
	("Lãng Mạn", "lang-man"),
	("Khoa Học Viễn Tưởng", "khoa-hoc-vien-tuong"),
	("Gây Cấn", "gay-can"),
	("Chiến Tranh", "chien-tranh"),
	("Tâm Lý", "tam-ly"),
	("Tình Cảm", "tinh-cam"),
	("Cổ Trang", "co-trang"),
	("Miền Tây", "mien-tay"),
	("Phim 18+", "phim-18"),
];

const COUNTRIES: &[(&str, &str)] = &[
	("Âu Mỹ", "au-my"),
	("Anh", "anh"),
	("Trung Quốc", "trung-quoc"),
	("Indonesia", "indonesia"),
	("Việt Nam", "viet-nam"),
	("Pháp", "phap"),
	("Hồng Kông", "hong-kong"),
	("Hàn Quốc", "han-quoc"),
	("Nhật Bản", "nhat-ban"),
	("Thái Lan", "thai-lan"),
	("Đài Loan", "dai-loan"),
	("Nga", "nga"),
	("Hà Lan", "ha-lan"),
	("Philippines", "philippines"),
	("Ấn Độ", "an-do"),
	("Quốc gia khác", "quoc-gia-khac"),
];

const YEAR_FIRST: i32 = 2016;
const YEAR_LAST: i32 = 2026;

fn year_options() -> Vec<String> {
	(YEAR_FIRST..=YEAR_LAST).map(|y| format!("{y}")).collect()
}

/// `danh-sach` category slugs for the "Loại phim" select + listings.
const TYPE_SLUGS: &[(&str, &str)] = &[
	("Phim bộ", "phim-bo"),
	("Phim lẻ", "phim-le"),
	("TV Shows", "tv-shows"),
	("Đang chiếu", "dang-chieu"),
];

// ────────────────────────────────────────────────────────────────────────────
// JSON structs — nguonc/Nguồn-family list/detail envelopes.
// ────────────────────────────────────────────────────────────────────────────

/// `total_episodes` arrives as a JSON number on every live endpoint; be
/// tolerant of a string representation in case a fork changes the shape.
fn de_flex_i32<'de, D>(deserializer: D) -> core::result::Result<i32, D::Error>
where
	D: serde::Deserializer<'de>,
{
	#[derive(Deserialize)]
	#[serde(untagged)]
	enum IntOrString {
		Int(i32),
		Str(String),
	}
	Ok(match IntOrString::deserialize(deserializer)? {
		IntOrString::Int(v) => v,
		IntOrString::Str(s) => s.trim().parse().unwrap_or(0),
	})
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Paginate {
	#[serde(rename = "current_page")]
	current_page: i32,
	#[serde(rename = "total_page")]
	total_page: i32,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct EpisodeEntry {
	name: String,
	slug: String,
	embed: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct EpisodeGroup {
	#[serde(rename = "server_name")]
	server_name: String,
	items: Vec<EpisodeEntry>,
}

/// A movie in a list or in the detail envelope (`movie`).
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct NguoncMovie {
	name: String,
	slug: String,
	#[serde(rename = "original_name")]
	original_name: String,
	#[serde(rename = "poster_url")]
	poster_url: Option<String>,
	#[serde(rename = "thumb_url")]
	thumb_url: Option<String>,
	description: Option<String>,
	#[serde(rename = "total_episodes", deserialize_with = "de_flex_i32")]
	total_episodes: i32,
	#[serde(rename = "current_episode")]
	current_episode: Option<String>,
	modified: Option<String>,
	quality: Option<String>,
	language: Option<String>,
	director: Option<String>,
	#[allow(dead_code)]
	casts: Option<String>,
	year: Option<String>,
	/// Object of `{ "<n>": { group: {name}, list: [{name, id}] } }` groups —
	/// "Định dạng", "Thể loại", "Năm", "Quốc gia". Kept as a raw JSON value and
	/// walked with [category_names] (serde's `Vec<(K,V)>` impl does not accept
	/// object input, and the group keys are arbitrary server ids).
	category: Option<serde_json::Value>,
	episodes: Vec<EpisodeGroup>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct ListEnvelope {
	paginate: Option<Paginate>,
	items: Vec<NguoncMovie>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct DetailEnvelope {
	movie: Option<NguoncMovie>,
}

// ────────────────────────────────────────────────────────────────────────────
// Stream grant responses (the embed player's own JSON API).
// ────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct BootstrapResp {
	bootstrap: Option<String>,
	#[serde(rename = "turnstileEnabled")]
	turnstile_enabled: bool,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct IssueResp {
	playlist: Option<String>,
	#[serde(rename = "playlistFormat")]
	playlist_format: Option<String>,
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

/// Extract the scheme+host origin of an embed url (`https://embed13.streamc.xyz`
/// from `…/embed.php?hash=…`). The grant POSTs and the media `Referer` all
/// need it.
fn embed_origin(url: &str) -> Option<String> {
	let (scheme, rest) = if let Some(rest) = url.strip_prefix("https://") {
		("https://", rest)
	} else if let Some(rest) = url.strip_prefix("http://") {
		("http://", rest)
	} else {
		return None;
	};
	let host = rest
		.split(['/', '?', '#'])
		.next()
		.filter(|h| !h.is_empty())?;
	Some(format!("{scheme}{host}"))
}

fn absolutize_opt(url: Option<&str>) -> Option<String> {
	url.map(|u| u.to_string()).filter(|s| !s.is_empty())
}

/// Extract the first run of digits in a string ("Tập 24" → "24", "Full" → "1").
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

/// Extract the first run of digits as an integer ("2026" → 2026).
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

/// Parse `"2026-09-21T16:47:48.000000Z"` → epoch millis (fraction can be any
/// digit length; truncated/padded to millis).
fn parse_isodate_millis(input: &str) -> Option<i64> {
	let bytes = input.as_bytes();
	if bytes.len() < 19 {
		return None;
	}
	let num = |start: usize, len: usize| -> Option<i64> {
		input[start..start + len].parse::<i64>().ok()
	};
	let y = num(0, 4)?;
	let mo = num(5, 2)?;
	let d = num(8, 2)?;
	let h = num(11, 2)?;
	let mi = num(14, 2)?;
	let s = num(17, 2)?;

	let mut ms = 0_i64;
	if bytes.len() >= 20 && bytes[19] == b'.' {
		let frac: String = input[20..]
			.chars()
			.take_while(|c| c.is_ascii_digit())
			.collect();
		let mut normalized = frac;
		if !normalized.is_empty() {
			while normalized.len() < 3 {
				normalized.push('0');
			}
			normalized.truncate(3);
			ms = normalized.parse().unwrap_or(0);
		}
	}

	let days = days_from_civil(y, mo, d);
	Some((days * 86_400 + h * 3_600 + mi * 60 + s) * 1_000 + ms)
}

/// Names of a `category` group by name ("Thể loại", "Quốc gia", "Năm",
/// "Định dạng"). Walks the `{ "<id>": { group: {name}, list: [{name, id}] } }`
/// JSON object directly.
fn category_names(category: Option<&serde_json::Value>, group_name: &str) -> Vec<String> {
	let Some(obj) = category.and_then(|c| c.as_object()) else {
		return Vec::new();
	};
	obj.values()
		.filter_map(|group| {
			let gname = group
				.get("group")
				.and_then(|g| g.get("name"))
				.and_then(|n| n.as_str())?;
			if gname != group_name {
				return None;
			}
			group
				.get("list")
				.and_then(|l| l.as_array())
				.map(|arr| {
					arr.iter()
						.filter_map(|item| {
							item.get("name")
								.and_then(|n| n.as_str())
								.map(String::from)
								.filter(|n| !n.is_empty())
						})
						.collect::<Vec<String>>()
				})
		})
		.flatten()
		.collect()
}

fn names_to_links(values: Vec<String>) -> Vec<CategoryLink> {
	values
		.into_iter()
		.map(|name| CategoryLink {
			name,
			filters: Vec::new(),
		})
		.collect()
}

fn map_status(movie: &NguoncMovie) -> AnimeStatus {
	let formats = category_names(movie.category.as_ref(), "Định dạng");
	let lower: Vec<String> = formats.iter().map(|s| s.to_lowercase()).collect();
	if lower.iter().any(|f| f.contains("hoàn thành") || f.contains("hoàn tất")) {
		AnimeStatus::Completed
	} else if lower.iter().any(|f| f.contains("đang chiếu")) {
		AnimeStatus::Ongoing
	} else {
		AnimeStatus::Unknown
	}
}

/// Strip Vietnamese diacritics from a char (best effort for the slug
/// fallback — the catalog tables already carry the exact API slugs).
fn strip_vn_diacritics(c: char) -> char {
	const A: &str = "àảãáạằẳẵắặầẩẫấậ";
	const E: &str = "èẻẽéẹềểễếệ";
	const I: &str = "ìỉĩíị";
	const O: &str = "òỏõóọồổỗốộờởỡớợ";
	const U: &str = "ùủũúụừửữứự";
	const Y: &str = "ỳỷỹýỵ";
	if A.contains(c) {
		'a'
	} else if E.contains(c) {
		'e'
	} else if I.contains(c) {
		'i'
	} else if O.contains(c) {
		'o'
	} else if U.contains(c) {
		'u'
	} else if Y.contains(c) {
		'y'
	} else if c == 'đ' {
		'd'
	} else {
		c
	}
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

/// Slugify fallback: lowercase, strip Vietnamese diacritics, keep ASCII
/// alphanumerics, everything else → `-`.
fn slugify(name: &str) -> String {
	let mut slug = String::new();
	let mut prev_dash = false;
	for c in name.chars() {
		let c = strip_vn_diacritics(c.to_lowercase().next().unwrap_or(c));
		if c.is_ascii_alphanumeric() {
			slug.push(c);
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

/// Pull a `FilterValue::Select`'s selected value by id.
fn select_value<'a>(filters: &'a [FilterValue], id: &str) -> Option<&'a str> {
	filters.iter().find_map(|f| match f {
		FilterValue::Select {
			id: f_id,
			value,
		} if f_id == id => Some(value.as_str()),
		_ => None,
	})
}

/// Pull the first selected (included) multi-select entry by id.
fn multi_included<'a>(filters: &'a [FilterValue], id: &str) -> Option<&'a str> {
	filters.iter().find_map(|f| match f {
		FilterValue::MultiSelect {
			id: f_id,
			included,
			..
		} if f_id == id => included.first().map(String::as_str),
		_ => None,
	})
}

// ────────────────────────────────────────────────────────────────────────────
// Fetch + build
// ────────────────────────────────────────────────────────────────────────────

/// Fetch one list page and return (items, has_next_page).
fn fetch_page(url: &str) -> Result<(Vec<NguoncMovie>, bool)> {
	let env: ListEnvelope = Request::get(url)?.json_owned()?;
	let pag = env.paginate.as_ref();
	let has_next = pag.map(|p| p.current_page < p.total_page).unwrap_or(false);
	Ok((env.items, has_next))
}

/// Fetch movie detail (`{base}/api/film/{slug}`).
fn fetch_detail(base: &str, slug: &str) -> Result<NguoncMovie> {
	let env: DetailEnvelope = Request::get(format!("{base}/api/film/{slug}"))?.json_owned()?;
	env.movie
		.ok_or_else(|| error!("Không tìm thấy phim: {slug}"))
}

/// Build the list endpoint for search/filtered/latest with the given page.
fn build_list_url(
	base: &str,
	query: Option<&str>,
	page: i32,
	filters: &[FilterValue],
) -> String {
	if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) {
		return format!(
			"{base}/api/films/search?keyword={}&page={page}",
			encode_uri_component(q)
		);
	}
	// The API lists are single-dimension (no combined genre+country+year
	// endpoint), so the first selected filter wins: type → genre → country →
	// year. This mirrors the visible site: `/the-loai/…`, `/quoc-gia/…`,
	// `/nam-phat-hanh/…` and `/danh-sach/…`.
	if let Some(t) = select_value(filters, "type")
		&& let Some((_, slug)) = TYPE_SLUGS
			.iter()
			.find(|(name, _)| name.eq_ignore_ascii_case(t))
	{
		return format!("{base}/api/films/danh-sach/{slug}?page={page}");
	}
	if let Some(g) = multi_included(filters, "genre") {
		return format!("{base}/api/films/the-loai/{}?page={page}", genre_slug(g));
	}
	if let Some(c) = multi_included(filters, "country") {
		return format!("{base}/api/films/quoc-gia/{}?page={page}", country_slug(c));
	}
	if let Some(y) = select_value(filters, "year")
		&& let Some(year) = parse_first_int(Some(y))
	{
		return format!("{base}/api/films/nam-phat-hanh/{year}?page={page}");
	}
	format!("{base}/api/films/phim-moi-cap-nhat?page={page}")
}

/// Build a Lite card from a list item.
fn build_lite(m: &NguoncMovie, base: &str) -> Anime {
	let poster = absolutize_opt(m.poster_url.as_deref().or(m.thumb_url.as_deref()));
	Anime {
		key: m.slug.clone(),
		source_id: SOURCE_ID.into(),
		title: m.name.clone(),
		original_title: m.original_name.clone(),
		cover: poster.clone().unwrap_or_default(),
		banner: poster,
		description: None,
		episode_count: m.total_episodes,
		current_episode: m.current_episode.clone(),
		rating: None,
		rating_count: None,
		status: AnimeStatus::Unknown,
		release_year: parse_first_int(m.year.as_deref()).map(|y| CategoryLink {
			name: format!("{y}"),
			filters: Vec::new(),
		}),
		genres: Vec::new(),
		authors: Vec::new(),
		studio: None,
		season_of: None,
		countries: Vec::new(),
		is_featured: false,
		views: 0,
		next_episode_air_info: None,
		quality_tag: m.quality.clone(),
		seasons: Vec::new(),
		episodes: None,
		url: Some(format!("{base}/phim/{}", m.slug)),
	}
}

/// Full detail: `seasons` = playback servers (sorted by episode count desc),
/// metadata from the `category` groups.
fn build_full(base: &str, m: &NguoncMovie, slug: &str) -> Anime {
	let mut groups: Vec<&EpisodeGroup> = m
		.episodes
		.iter()
		.filter(|g| !g.items.is_empty())
		.collect();
	groups.sort_by_key(|g| core::cmp::Reverse(g.items.len()));

	let seasons = groups
		.iter()
		.map(|g| AnimeSeason {
			anime_id: format!("{slug}|{}", g.server_name),
			title: g.server_name.clone(),
			id: format!("{slug}|{}", g.server_name),
		})
		.collect();

	let poster = absolutize_opt(m.poster_url.as_deref());
	let thumb = absolutize_opt(m.thumb_url.as_deref());
	let cover = poster.clone().or_else(|| thumb.clone());

	let category = m.category.as_ref();
	let status = map_status(m);

	let episode_count = groups
		.first()
		.map(|g| g.items.len() as i32)
		.unwrap_or(m.total_episodes);

	// Detail responses carry the year inside the "Năm" category group; lists
	// carry it as a top-level string. Prefer the category value.
	let year_name = category_names(category, "Năm").first().cloned();
	let release_year =
		year_name
			.or_else(|| m.year.clone())
			.and_then(|y| parse_first_int(Some(&y)))
			.map(|y| CategoryLink {
				name: format!("{y}"),
				filters: Vec::new(),
			});

	Anime {
		key: slug.to_string(),
		source_id: SOURCE_ID.into(),
		title: m.name.clone(),
		original_title: m.original_name.clone(),
		cover: cover.unwrap_or_default(),
		banner: poster,
		description: Some(m.description.clone().unwrap_or_default()),
		episode_count,
		current_episode: m.current_episode.clone(),
		rating: None,
		rating_count: None,
		status,
		release_year,
		genres: names_to_links(category_names(category, "Thể loại")),
		authors: m
			.director
			.as_deref()
			.filter(|d| !d.trim().is_empty())
			.map(|d| vec![CategoryLink {
				name: d.trim().to_string(),
				filters: Vec::new(),
			}])
			.unwrap_or_default(),
		studio: None,
		season_of: None,
		countries: names_to_links(category_names(category, "Quốc gia")),
		is_featured: false,
		views: 0,
		next_episode_air_info: None,
		quality_tag: m.quality.clone(),
		seasons,
		episodes: None,
		url: Some(format!("{base}/phim/{slug}")),
	}
}

/// Stub episode for the Home "Mới Cập Nhật" row — count + date from the item.
fn home_episode(m: &NguoncMovie) -> Episode {
	Episode {
		key: format!("{}__latest", m.slug),
		episode_number: parse_episode_number(m.current_episode.as_deref().unwrap_or("1")),
		title: Some(m.name.clone()),
		date_uploaded: m.modified.as_deref().and_then(parse_isodate_millis),
		thumbnail: absolutize_opt(m.thumb_url.as_deref().or(m.poster_url.as_deref())),
		..Default::default()
	}
}

/// Episodes for the selected server (None → first server = largest).
fn episodes_for_server(groups: &[EpisodeGroup], server: Option<&str>) -> Vec<Episode> {
	let group = match server {
		Some(name) => groups
			.iter()
			.find(|g| g.server_name.eq_ignore_ascii_case(name))
			.or_else(|| groups.first()),
		None => groups.first(),
	};
	match group {
		Some(g) => g
			.items
			.iter()
			.map(|it| Episode {
				key: it.slug.clone(),
				episode_number: parse_episode_number(&it.name),
				title: None,
				date_uploaded: None,
				..Default::default()
			})
			.collect(),
		None => Vec::new(),
	}
}

/// Find the episode entry: prefer the requested server, fall back to the
/// first group containing the episode key.
fn find_episode<'a>(
	groups: &'a [EpisodeGroup],
	server: &str,
	episode_key: &str,
) -> Option<&'a EpisodeEntry> {
	groups
		.iter()
		.find(|g| g.server_name.eq_ignore_ascii_case(server))
		.and_then(|g| g.items.iter().find(|e| e.slug == episode_key))
		.or_else(|| {
			groups
				.iter()
				.flat_map(|g| g.items.iter())
				.find(|e| e.slug == episode_key)
		})
}

/// POST a JSON body and deserialize the response into `T`. On a non-2xx it
/// surfaces the server's `{ "error": … }` message.
fn post_json<T: serde::de::DeserializeOwned>(url: &str, body: &str, origin: &str) -> Result<T> {
	let request = match Request::post(url) {
		Ok(r) => r,
		Err(_) => return Err(error!("Địa chỉ nguồn phát không hợp lệ.")),
	};
	let request = request
		.header("Content-Type", "application/json")
		.header("Origin", origin)
		.header("Referer", origin)
		.body(body);
	let resp = match request.send() {
		Ok(r) => r,
		Err(_) => return Err(error!("Không kết nối được máy chủ nguồn phát.")),
	};
	let status = resp.status_code();
	if status < 200 || status >= 300 {
		let detail = resp.get_string().unwrap_or_default();
		let server_msg = serde_json::from_str::<serde_json::Value>(&detail)
			.ok()
			.and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
			.map(|m| format!(": {m}"))
			.unwrap_or_default();
		bail!("Máy chủ nguồn phát lỗi {status}{server_msg}");
	}
	resp
		.get_json_owned::<T>()
		.map_err(|_| error!("Máy chủ nguồn phát trả dữ liệu không hợp lệ."))
}

/// Walk the embed player's two-step grant and return the signed HLS playlist
/// URL. `referrer` is the film page the embed would be loaded from;
/// `origin` is passed as both `Origin`/`Referer` on the POSTs and becomes the
/// media `Referer` for the segment CDN.
fn resolve_playlist(embed_url: &str, referrer: &str, origin: &str) -> Result<String> {
	let bootstrap_body = serde_json::json!({
		"action": "bootstrap",
		"referrer": referrer,
		"frame_origins": [origin],
		"request_grant": true,
		"playlist_format": PLAYLIST_FORMAT,
		"pretty_url": true,
		"path_chunks": true,
		"bootstrap_format": "json",
	});
	let bootstrap = post_json::<BootstrapResp>(embed_url, &bootstrap_body.to_string(), origin)?;
	let token = bootstrap
		.bootstrap
		.ok_or_else(|| error!("Không nhận được quyền phát từ máy chủ nguồn."))?;

	let issue_body = serde_json::json!({
		"action": "issue",
		"bootstrap": token,
		"turnstile_response": "",
		"playlist_format": PLAYLIST_FORMAT,
		"pretty_url": true,
		"path_chunks": true,
		"frame_origins": [origin],
	});
	let issue = post_json::<IssueResp>(embed_url, &issue_body.to_string(), origin)?;
	let playlist = issue
		.playlist
		.ok_or_else(|| error!("Không lấy được nguồn phát video."))?;

	if let Some(fmt) = issue.playlist_format.as_deref() && fmt != PLAYLIST_FORMAT {
		bail!("Nguồn phát dùng định dạng mã hoá ({fmt}), app chưa hỗ trợ.");
	}
	Ok(playlist)
}

// ────────────────────────────────────────────────────────────────────────────
// Source
// ────────────────────────────────────────────────────────────────────────────

pub struct NguonphimSource;

impl NguonphimSource {
	fn base(&self) -> String {
		match defaults_get::<String>(SETTING_BASE_URL) {
			Some(s) if !s.trim().is_empty() => s.trim().to_string(),
			_ => String::from(DEFAULT_BASE),
		}
	}
}

impl Source for NguonphimSource {
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
		let url = build_list_url(&base, query.as_deref(), page, &filters);
		let (items, has_next) = fetch_page(&url)?;
		Ok(AnimePageResult {
			entries: items.iter().map(|m| build_lite(m, &base)).collect(),
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
			let movie = fetch_detail(&self.base(), slug)?;
			if needs_details {
				let full = build_full(&self.base(), &movie, slug);
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

		let entry = find_episode(&movie.episodes, &stream.key, &episode.key).ok_or_else(|| {
			error!(
				"Nguồn phát {} không có tập {} ({}).",
				stream.key, episode.episode_number, episode.key
			)
		})?;

		let Some(embed) = entry.embed.as_deref() else {
			bail!("Không tìm thấy liên kết phát cho tập {}.", episode.episode_number);
		};

		let Some(origin) = embed_origin(embed) else {
			bail!("Liên kết phát không hợp lệ: {embed}");
		};

		let referrer = format!("{base}/phim/{slug}");
		let playlist = resolve_playlist(embed, &referrer, &origin)?;

		let mut headers = komorei::HashMap::new();
		headers.insert(String::from("Referer"), format!("{origin}/"));

		Ok(StreamData {
			url: playlist,
			stream_type: StreamType::HLS,
			is_content: false,
			headers,
			subtitles: Vec::new(),
			intro: None,
			outro: None,
		})
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Listings
// ────────────────────────────────────────────────────────────────────────────

impl ListingProvider for NguonphimSource {
	fn get_anime_list(&self, listing: Listing, page: i32) -> Result<AnimePageResult> {
		if !matches!(listing.kind, ListingKind::List) {
			return Ok(AnimePageResult::default());
		}
		let base = self.base();
		let url = match listing.id.as_str() {
			"latest" => format!("{base}/api/films/phim-moi-cap-nhat?page={page}"),
			id => {
				let path = TYPE_SLUGS
					.iter()
					.find(|(_, slug)| *slug == id)
					.map(|(_, slug)| *slug);
				match path {
					Some(p) => format!("{base}/api/films/danh-sach/{p}?page={page}"),
					// Unknown rail id → the top-level latest endpoint (a
					// `danh-sach/{unknown}` URL would 404 on the API).
					None => format!("{base}/api/films/phim-moi-cap-nhat?page={page}"),
				}
			}
		};
		let (items, has_next) = fetch_page(&url)?;
		Ok(AnimePageResult {
			entries: items.iter().map(|m| build_lite(m, &base)).collect(),
			has_next_page: has_next,
		})
	}
}

impl DynamicListings for NguonphimSource {
	fn get_dynamic_listings(&self) -> Result<Vec<Listing>> {
		Ok(vec![
			Listing {
				id: String::from("latest"),
				name: String::from("Mới Cập Nhật"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("dang-chieu"),
				name: String::from("Đang Chiếu"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("phim-bo"),
				name: String::from("Phim Bộ"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("phim-le"),
				name: String::from("Phim Lẻ"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("tv-shows"),
				name: String::from("TV Shows"),
				kind: ListingKind::List,
			},
		])
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Home
// ────────────────────────────────────────────────────────────────────────────

impl Home for NguonphimSource {
	fn get_home(&self) -> Result<HomeLayout> {
		let base = self.base();
		let urls = [
			format!("{base}/api/films/phim-moi-cap-nhat?page=1"),
			format!("{base}/api/films/danh-sach/phim-bo?page=1"),
			format!("{base}/api/films/danh-sach/phim-le?page=1"),
		];

		// 3 parallel requests for the home rows.
		let mut requests = Vec::with_capacity(3);
		for u in &urls {
			requests.push(Request::get(u.as_str())?);
		}
		let responses = Request::send_all(requests);

		let mut latest: Vec<NguoncMovie> = Vec::new();
		let mut bo: Vec<NguoncMovie> = Vec::new();
		let mut le: Vec<NguoncMovie> = Vec::new();
		for (i, resp) in responses.into_iter().enumerate() {
			let Ok(resp) = resp else { continue };
			let Ok(env) = resp.get_json_owned::<ListEnvelope>() else {
				continue;
			};
			let items = env.items;
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
					image_url: absolutize_opt(m.poster_url.as_deref().or(m.thumb_url.as_deref())),
					value: Some(LinkValue::Anime(build_lite(m, &base))),
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
					anime: build_lite(m, &base),
					episode: home_episode(m),
				})
				.collect();
			components.push(HomeComponent {
				title: Some(String::from("Mới Cập Nhật")),
				value: HomeComponentValue::AnimeEpisodeList {
					page_size: None,
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
					subtitle: m.current_episode.clone(),
					image_url: absolutize_opt(m.poster_url.as_deref().or(m.thumb_url.as_deref())),
					value: Some(LinkValue::Anime(build_lite(m, &base))),
				})
				.collect();
			components.push(HomeComponent {
				title: Some(String::from("Phim Bộ")),
				value: HomeComponentValue::Scroller {
					entries,
					listing: Some(Listing {
						id: String::from("phim-bo"),
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
					subtitle: m.current_episode.clone(),
					image_url: absolutize_opt(m.poster_url.as_deref().or(m.thumb_url.as_deref())),
					value: Some(LinkValue::Anime(build_lite(m, &base))),
				})
				.collect();
			components.push(HomeComponent {
				title: Some(String::from("Phim Lẻ")),
				value: HomeComponentValue::AnimeList {
					ranking: false, /* list is ordered by update, not popularity */
					page_size: None,
					entries,
					listing: Some(Listing {
						id: String::from("phim-le"),
						name: String::from("Phim Lẻ"),
						kind: ListingKind::List,
					}),
				},
				..Default::default()
			});
		}

		// Genre chips → search with the "genre" filter
		let filters: Vec<FilterItem> = GENRES
			.iter()
			.map(|(name, _)| FilterItem {
				title: name.to_string(),
				values: Some(vec![FilterValue::MultiSelect {
					id: String::from("genre"),
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
			("dang-chieu", "Đang Chiếu"),
			("phim-bo", "Phim Bộ"),
			("phim-le", "Phim Lẻ"),
			("tv-shows", "TV Shows"),
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

impl DynamicFilters for NguonphimSource {
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
			SelectFilter {
				id: "type".into(),
				title: Some("Loại phim".into()),
				options: {
					let mut o: Vec<Cow<'static, str>> = vec!["Tất cả".into()];
					o.extend(TYPE_SLUGS.iter().map(|(n, _)| Cow::Borrowed(*n)));
					o
				},
				..Default::default()
			}
			.into(),
			MultiSelectFilter {
				id: "genre".into(),
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
				id: "year".into(),
				title: Some("Năm phát hành".into()),
				options: {
					let mut o: Vec<Cow<'static, str>> = vec!["Tất cả".into()];
					o.extend(year_options().into_iter().map(Cow::Owned));
					o
				},
				..Default::default()
			}
			.into(),
			Filter::note(
				"Nguồn dữ liệu từ Nguồn Phim (https://api.nguonphim.net). Link phát được cấp quyền theo tập qua máy chủ embed.",
			),
		])
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Settings
// ────────────────────────────────────────────────────────────────────────────

impl DynamicSettings for NguonphimSource {
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		Ok(vec![
			TextSetting {
				key: SETTING_BASE_URL.into(),
				title: "Địa chỉ API Nguồn Phim".into(),
				placeholder: Some(DEFAULT_BASE.into()),
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

impl NotificationHandler for NguonphimSource {
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

impl DeepLinkHandler for NguonphimSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
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
// Migration — Nguồn-family ids are stable slugs, keep identity.
// ────────────────────────────────────────────────────────────────────────────

impl MigrationHandler for NguonphimSource {
	fn handle_anime_migration(&self, key: String) -> Result<String> {
		Ok(key)
	}

	fn handle_episode_migration(&self, _anime_key: String, episode_key: String) -> Result<String> {
		Ok(episode_key)
	}
}

register_source!(
	NguonphimSource,
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

	fn sample_list_json() -> &'static str {
		r#"{
			"status": "success",
			"paginate": { "current_page": 1, "total_page": 3354, "items_per_page": 10 },
			"items": [{
				"name": "Lan Hương Như Cố",
				"slug": "lan-huong-nhu-co",
				"original_name": "Against The Current",
				"thumb_url": "https://phim.nguonc.com/public/images/Post/2/lan-huong-nhu-co.jpg",
				"poster_url": "https://phim.nguonc.com/public/images/Post/2/lan-huong-nhu-co-1.jpg",
				"modified": "2026-09-21T16:28:46.000000Z",
				"description": "Thẩm Gia Lan…",
				"total_episodes": 47,
				"current_episode": "Tập 24",
				"time": "46 Phút/Tập",
				"quality": "HD",
				"language": "Vietsub + Thuyết Minh",
				"director": "Hoàng Dĩnh Tương",
				"year": "2026"
			}]
		}"#
	}

	#[komorei_test]
	fn parses_list_envelope_and_builds_lite() {
		let env: ListEnvelope = serde_json::from_str(sample_list_json()).expect("list decodes");
		assert_eq!(env.items.len(), 1);
		let m = &env.items[0];
		assert_eq!(m.slug, "lan-huong-nhu-co");
		assert_eq!(m.total_episodes, 47);
		assert_eq!(m.quality.as_deref(), Some("HD"));

		let lite = build_lite(m, DEFAULT_BASE);
		assert_eq!(lite.key, "lan-huong-nhu-co");
		assert_eq!(lite.title, "Lan Hương Như Cố");
		assert_eq!(lite.original_title, "Against The Current");
		assert_eq!(lite.episode_count, 47);
		assert_eq!(lite.release_year.as_ref().map(|y| y.name.as_str()), Some("2026"));
		assert_eq!(lite.current_episode.as_deref(), Some("Tập 24"));
		assert_eq!(lite.quality_tag.as_deref(), Some("HD"));
		assert_eq!(
			lite.cover,
			"https://phim.nguonc.com/public/images/Post/2/lan-huong-nhu-co-1.jpg"
		);
	}

	#[komorei_test]
	fn tolerates_string_total_episodes() {
		let json = r#"{
			"status": "success",
			"paginate": { "current_page": 1, "total_page": 2 },
			"items": [{ "name": "Ký Sinh Trùng", "slug": "ky-sinh-trung", "total_episodes": "1" }]
		}"#;
		let env: ListEnvelope = serde_json::from_str(json).expect("string episodes decode");
		assert_eq!(env.items[0].total_episodes, 1);
	}

	#[komorei_test]
	fn computes_has_next_from_paginate() {
		let env: ListEnvelope = serde_json::from_str(sample_list_json()).expect("list decodes");
		let pag = env.paginate.as_ref().unwrap();
		assert!(pag.current_page < pag.total_page);
	}

	#[komorei_test]
	fn parses_detail_category_and_builds_full() {
		let json = r#"{
			"status": "success",
			"movie": {
				"name": "Lan Hương Như Cố",
				"slug": "lan-huong-nhu-co",
				"original_name": "Against The Current",
				"thumb_url": "https://phim.nguonc.com/t.jpg",
				"poster_url": "https://phim.nguonc.com/p.jpg",
				"description": "Mô tả phim.",
				"total_episodes": 47,
				"current_episode": "Tập 24",
				"quality": "HD",
				"category": {
					"1": { "group": { "id": "c4", "name": "Định dạng" }, "list": [ { "id": "x", "name": "Phim bộ" }, { "id": "y", "name": "Đang chiếu" } ] },
					"2": { "group": { "id": "c5", "name": "Thể loại" }, "list": [ { "id": "a", "name": "Chính Kịch" }, { "id": "b", "name": "Cổ Trang" } ] },
					"3": { "group": { "id": "c6", "name": "Năm" }, "list": [ { "id": "c", "name": "2026" } ] },
					"4": { "group": { "id": "c7", "name": "Quốc gia" }, "list": [ { "id": "d", "name": "Trung Quốc" } ] }
				},
				"episodes": [
					{ "server_name": "Thuyết minh #1", "items": [ { "name": "1", "slug": "tap-1", "embed": "https://embed12.streamc.xyz/embed.php?hash=abc1" } ] },
					{ "server_name": "Vietsub #1", "items": [
						{ "name": "1", "slug": "tap-1", "embed": "https://embed13.streamc.xyz/embed.php?hash=abc2" },
						{ "name": "2", "slug": "tap-2", "embed": "https://embed13.streamc.xyz/embed.php?hash=abc3" }
					] }
				]
			}
		}"#;
		let env: DetailEnvelope = serde_json::from_str(json).expect("detail decodes");
		let m = env.movie.expect("movie present");

		// category object → walked as JSON groups
		assert_eq!(category_names(m.category.as_ref(), "Thể loại"), vec!["Chính Kịch", "Cổ Trang"]);
		assert_eq!(category_names(m.category.as_ref(), "Quốc gia"), vec!["Trung Quốc"]);
		assert_eq!(category_names(m.category.as_ref(), "Năm"), vec!["2026"]);
		assert_eq!(map_status(&m), AnimeStatus::Ongoing);

		let full = build_full(DEFAULT_BASE, &m, "lan-huong-nhu-co");
		assert_eq!(full.title, "Lan Hương Như Cố");
		assert_eq!(full.description.as_deref(), Some("Mô tả phim."));
		assert_eq!(full.status, AnimeStatus::Ongoing);
		assert_eq!(full.release_year.as_ref().map(|y| y.name.as_str()), Some("2026"));
		assert_eq!(full.genres.len(), 2);
		assert_eq!(full.genres[0].name, "Chính Kịch");
		assert_eq!(full.countries.len(), 1);
		assert_eq!(full.authors.len(), 0);

		// servers sorted by episode count descending → Vietsub (2) first
		assert_eq!(
			full.seasons
				.iter()
				.map(|s| s.anime_id.as_str())
				.collect::<Vec<_>>(),
			vec!["lan-huong-nhu-co|Vietsub #1", "lan-huong-nhu-co|Thuyết minh #1"]
		);
		assert_eq!(full.episode_count, 2);
	}

	#[komorei_test]
	fn episodes_for_server_respects_selected_server() {
		let groups = vec![
			EpisodeGroup {
				server_name: String::from("Thuyết minh #1"),
				items: vec![EpisodeEntry {
					slug: String::from("tap-1"),
					..Default::default()
				}],
			},
			EpisodeGroup {
				server_name: String::from("Vietsub #1"),
				items: vec![
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
		];
		// explicit server
		assert_eq!(
			episodes_for_server(&groups, Some("Vietsub #1"))
				.iter()
				.map(|e| e.key.as_str())
				.collect::<Vec<_>>(),
			vec!["tap-1", "tap-2"]
		);
		// default (no server) → first group
		assert_eq!(
			episodes_for_server(&groups, None)
				.iter()
				.map(|e| e.key.as_str())
				.collect::<Vec<_>>(),
			vec!["tap-1"]
		);
		// case-insensitive server match
		assert_eq!(
			episodes_for_server(&groups, Some("thuyết minh #1")).len(),
			1
		);
	}

	#[komorei_test]
	fn parses_embed_origin() {
		assert_eq!(
			embed_origin("https://embed13.streamc.xyz/embed.php?hash=abc"),
			Some(String::from("https://embed13.streamc.xyz"))
		);
		assert_eq!(
			embed_origin("https://embed12.streamc.xyz/embed.php?hash=abc&x=1"),
			Some(String::from("https://embed12.streamc.xyz"))
		);
		assert_eq!(embed_origin("not-a-url"), None);
		assert_eq!(embed_origin("https:///no-host"), None);
	}

	#[komorei_test]
	fn finds_episode_with_server_fallback() {
		let groups = vec![
			EpisodeGroup {
				server_name: String::from("Vietsub #1"),
				items: vec![EpisodeEntry {
					slug: String::from("tap-1"),
					embed: Some(String::from("https://embed13.streamc.xyz/embed.php?hash=a")),
					..Default::default()
				}],
			},
			EpisodeGroup {
				server_name: String::from("Thuyết minh #1"),
				items: vec![EpisodeEntry {
					slug: String::from("tap-1"),
					embed: Some(String::from("https://embed12.streamc.xyz/embed.php?hash=b")),
					..Default::default()
				}],
			},
		];
		// exact server wins
		let hit = find_episode(&groups, "thuyết minh #1", "tap-1").expect("found");
		assert_eq!(hit.embed.as_deref(), Some("https://embed12.streamc.xyz/embed.php?hash=b"));
		// missing on requested server → first group containing the episode
		let fallback = find_episode(&groups, "server không tồn tại", "tap-1").expect("fallback");
		assert_eq!(fallback.embed.as_deref(), Some("https://embed13.streamc.xyz/embed.php?hash=a"));
		// episode missing everywhere
		assert!(find_episode(&groups, "Vietsub #1", "tap-99").is_none());
	}

	#[komorei_test]
	fn builds_list_urls_for_search_and_filters() {
		let base = DEFAULT_BASE;
		// query beats filters
		assert_eq!(
			build_list_url(base, Some("Regeneration"), 1, &[]),
			format!("{base}/api/films/search?keyword=Regeneration&page=1")
		);
		// query is percent-encoded
		assert_eq!(
			build_list_url(base, Some("phim hay 2"), 1, &[]),
			format!("{base}/api/films/search?keyword=phim%20hay%202&page=1")
		);
		// type wins over genre/country/year
		let filters = vec![
			FilterValue::MultiSelect {
				id: String::from("genre"),
				included: vec![String::from("Kinh Dị")],
				excluded: Vec::new(),
			},
			FilterValue::Select {
				id: String::from("type"),
				value: String::from("Phim bộ"),
			},
		];
		assert_eq!(
			build_list_url(base, None, 3, &filters),
			format!("{base}/api/films/danh-sach/phim-bo?page=3")
		);
		// genre maps via the catalog table
		let genre_only = vec![FilterValue::MultiSelect {
			id: String::from("genre"),
			included: vec![String::from("Kinh Dị")],
			excluded: Vec::new(),
		}];
		assert_eq!(
			build_list_url(base, None, 2, &genre_only),
			format!("{base}/api/films/the-loai/kinh-di?page=2")
		);
		// country
		let country_only = vec![FilterValue::MultiSelect {
			id: String::from("country"),
			included: vec![String::from("Hàn Quốc")],
			excluded: Vec::new(),
		}];
		assert_eq!(
			build_list_url(base, None, 1, &country_only),
			format!("{base}/api/films/quoc-gia/han-quoc?page=1")
		);
		// year
		let year_only = vec![FilterValue::Select {
			id: String::from("year"),
			value: String::from("2024"),
		}];
		assert_eq!(
			build_list_url(base, None, 1, &year_only),
			format!("{base}/api/films/nam-phat-hanh/2024?page=1")
		);
		// nothing → latest
		assert_eq!(
			build_list_url(base, None, 1, &[]),
			format!("{base}/api/films/phim-moi-cap-nhat?page=1")
		);
	}

	#[komorei_test]
	fn slugifies_unlisted_genre_and_country_names() {
		assert_eq!(genre_slug("Khoa Học Viễn Tưởng"), "khoa-hoc-vien-tuong");
		assert_eq!(country_slug("Quốc gia khác"), "quoc-gia-khac");
		// fallback slugify for unknown names
		assert_eq!(genre_slug("Phim Mới 2026"), "phim-moi-2026");
	}

	#[komorei_test]
	fn parses_nanoseconds_fraction_to_millis() {
		assert_eq!(
			parse_isodate_millis("2026-09-21T16:28:46.000000Z"),
			Some(1_790_008_126_000)
		);
		assert_eq!(parse_isodate_millis("2026-09-21"), None);
		assert_eq!(parse_isodate_millis("junk"), None);
	}

	#[komorei_test]
	fn splits_season_key_into_slug_and_server() {
		assert_eq!(
			split_server("lan-huong-nhu-co|Vietsub #1"),
			("lan-huong-nhu-co", Some("Vietsub #1"))
		);
		assert_eq!(split_server("lan-huong-nhu-co"), ("lan-huong-nhu-co", None));
	}
}