//! # OPhim source (`vi.ophim`)
//!
//! Scrapes the OPhim API — the API family reused by OPhim clones and the
//! phimapi project (phimapi.com, ...). The original `ophim1.com` is dead ("the
//! website has been stopped"); the default base is the living `phimapi.com`.
//! The source reads BOTH envelope shapes:
//!
//! - classic (standard OPhim): `{ status, data: { items | item, params } }`
//! - flat fork (phimapi): list `{ status, items, pagination }`, search
//!   `{ status, data: { items } }`, detail `{ status, movie, episodes }`
//!   (**episodes at the ROOT**, not under `movie`)
//!
//! Because OPhim domains die and are reborn constantly, the base URL can be
//! changed in Source settings (`base_url`), defaulting to `https://phimapi.com`.
//!
//! ## Data model mapping to Komorei
//!
//! - **Season = one playback server** on the detail page. `AnimeSeason.anime_id`
//!   is encoded as `"{slug}|{server_name}"`; the app calls `get_anime_update`
//!   with that key (via `AnimeDetailViewModel.fetchEpisodesForSeason`) and the
//!   source strips the part after `|` to return the episodes of the selected
//!   server. Servers are sorted by episode count descending, so the largest one
//!   (usually `OPhim`) is the default.
//! - **Episode key = `slug` of the entry** in `server_data` (e.g. `tap-1`).
//! - `get_stream_list`: each server is a `StreamInfo` (taken from the `seasons`
//!   the app just received, no extra request). `get_stream`: refetches the
//!   detail, finds the group by server and the entry by episode key; when the
//!   requested server does not have that episode it **falls back to the first
//!   group containing it** (supports the app auto-resolving the first server
//!   for episodes that only exist on another server).

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
	Setting, SortFilter, SortFilterDefault, Source, StreamData, StreamInfo, StreamType, TextFilter,
	TextSetting,
	helpers::uri::QueryParameters,
	imports::defaults::{DefaultValue, defaults_get, defaults_set},
	imports::net::Request,
	prelude::*,
	serde::Deserialize,
};

const SOURCE_ID: &str = "vi.ophim";
const DEFAULT_BASE: &str = "https://phimapi.com";
/// Items per API page when there is no `pagination` (fallback for has_next).
const ITEMS_PER_PAGE: usize = 24;
const SETTING_BASE_URL: &str = "base_url";
const SETTING_LAST_NOTIFICATION: &str = "last_notification";

// ────────────────────────────────────────────────────────────────────────────
// Genre & Country catalogs (name → OPhim slug). Used for both the home
// "Thể Loại" chips and the MultiSelect filters — when building the search URL
// the name is mapped to a slug through these tables.
// ────────────────────────────────────────────────────────────────────────────

const GENRES: &[(&str, &str)] = &[
	("Hành Động", "hanh-dong"),
	("Cổ Trang", "co-trang"),
	("Chiến Tranh", "chien-tranh"),
	("Viễn Tưởng", "vien-tuong"),
	("Kinh Dị", "kinh-di"),
	("Tài Liệu", "tai-lieu"),
	("Tâm Lý", "tam-ly"),
	("Tình Cảm", "tinh-cam"),
	("Gia Đình", "gia-dinh"),
	("Hoạt Hình", "hoat-hinh"),
	("Thể Thao", "the-thao"),
	("Âm Nhạc", "am-nhac"),
	("Thế Giới Trẻ", "the-gioi-tre"),
	("Hình Sự", "hinh-su"),
	("Võ Thuật", "vo-thuat"),
	("Phiêu Lưu", "phieu-luu"),
	("Khoa Học", "khoa-hoc"),
	("Hài Hước", "hai-huoc"),
	("Bí Ẩn", "bi-an"),
	("Học Đường", "hoc-duong"),
	("Lãng Mạn", "lang-man"),
	("Chuyển Sinh", "chuyen-sinh"),
	("Mecha", "mecha"),
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
];

// ────────────────────────────────────────────────────────────────────────────
// JSON structs — deserialize both the classic OPhim and flat-fork envelopes.
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
	link_embed: Option<String>,
	link_m3u8: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct EpisodeGroup {
	server_name: String,
	server_data: Vec<EpisodeEntry>,
}

/// `episode_total`/`episode_current` on the OPhim detail — historically strings
/// (`"Tập 12/24"`, `"Full"`, `"Hoàn Tất (12/12)"`), but newer phimapi forks send
/// the raw integer (`25`, `12`). Accept both.
#[derive(Deserialize)]
#[serde(untagged)]
enum StrOrNum {
	Text(String),
	Number(i64),
}

/// Tolerant `Option<String>` deserializer: unwraps the integer form of
/// `episode_total`/`episode_current` into its string rendering so parsing never
/// trips on the string→number drift (serde errors with "invalid type: integer
/// `25`, expected a string" without this, killing the whole detail).
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

/// A movie in a list or detail (detail additionally has `content`, `episodes`).
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct OphimMovie {
	#[serde(rename = "_id")]
	id: Option<String>,
	name: String,
	origin_name: String,
	slug: String,
	poster_url: Option<String>,
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

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct ListData {
	items: Option<Vec<OphimMovie>>,
	params: Option<ListParams>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct ListEnvelope {
	data: Option<ListData>,
	items: Option<Vec<OphimMovie>>,
	/// phimapi list pagination lives at the ROOT (`{ status, items,
	/// pagination: {...} }`) instead of `data.params.pagination`.
	pagination: Option<Pagination>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct ListParams {
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
struct DetailData {
	item: Option<OphimMovie>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct DetailEnvelope {
	data: Option<DetailData>,
	movie: Option<OphimMovie>,
	/// phimapi detail puts the episode groups at the ROOT of the envelope
	/// (`{ status, msg, movie: {...}, episodes: [...] }`) while classic OPhim
	/// nests them under `data.item.episodes`.
	episodes: Option<Vec<EpisodeGroup>>,
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

/// Turn a protocol-relative (`//img.ophim...`) or relative path URL absolute.
fn absolutize_url(url: &str) -> String {
	if url.starts_with("//") {
		format!("https:{url}")
	} else if url.starts_with('/') {
		format!("{DEFAULT_BASE}{url}")
	} else {
		url.to_string()
	}
}

fn absolutize_opt(url: Option<&str>) -> Option<String> {
	url.map(absolutize_url).filter(|s| !s.is_empty())
}

/// Extract the first run of digits in a string ("Tập 12/24" → "12", "Full HD" → "1").
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

/// Parse `"2026-09-21T18:52:19.000Z"` → epoch millis (the app reads it via
/// `Instant.ofEpochMilli`).
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

/// Strip HTML tags from `content` (OPhim returns the description as HTML) and
/// decode a few common entities.
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

fn map_status(m: &OphimMovie) -> AnimeStatus {
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

// ────────────────────────────────────────────────────────────────────────────
// Fetch + build
// ────────────────────────────────────────────────────────────────────────────

/// Fetch movie detail (`{base}/phim/{slug}`), handling both envelopes.
fn fetch_detail(base: &str, slug: &str) -> Result<OphimMovie> {
	let env: DetailEnvelope = Request::get(format!("{base}/phim/{slug}"))?.json_owned()?;
	let mut movie = env
		.data
		.and_then(|d| d.item)
		.or(env.movie)
		.ok_or_else(|| error!("Không tìm thấy phim: {slug}"))?;
	// phimapi returns the episode groups at the envelope root; classic OPhim
	// already has them in `data.item.episodes`. Prefer the root list when the
	// nested one is empty (e.g. clones that only fill the top-level field).
	if movie.episodes.is_empty() {
		movie.episodes = env.episodes.unwrap_or_default();
	}
	Ok(movie)
}

/// Fetch one list page and return (items, has_next_page).
fn fetch_page(url: &str) -> Result<(Vec<OphimMovie>, bool)> {
	let env: ListEnvelope = Request::get(url)?.json_owned()?;
	let has_next = env
		.pagination
		.as_ref()
		.or_else(|| {
			env.data
				.as_ref()
				.and_then(|d| d.params.as_ref())
				.and_then(|p| p.pagination.as_ref())
		})
		.map(|pg| pg.current_page < pg.total_pages)
		.unwrap_or(false);
	let items = env
		.data
		.and_then(|d| d.items)
		.or(env.items)
		.unwrap_or_default();
	let has_next = has_next || items.len() >= ITEMS_PER_PAGE;
	Ok((items, has_next))
}

/// Build a Lite card from a list item.
fn build_lite(m: &OphimMovie, base: &str) -> Anime {
	let poster = absolutize_opt(m.poster_url.as_deref().or(m.thumb_url.as_deref()));
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
		url: Some(format!("{base}/phim/{}", m.slug)),
	}
}

/// Full detail: `seasons` = playback servers (sorted by episode count desc).
fn build_full(base: &str, m: &OphimMovie, slug: &str) -> Anime {
	let mut groups: Vec<&EpisodeGroup> = m
		.episodes
		.iter()
		.filter(|g| !g.server_data.is_empty())
		.collect();
	groups.sort_by_key(|g| core::cmp::Reverse(g.server_data.len()));

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
		url: Some(format!("{base}/phim/{slug}")),
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
fn home_episode(m: &OphimMovie) -> Episode {
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
		thumbnail: absolutize_opt(m.thumb_url.as_deref().or(m.poster_url.as_deref())),
		..Default::default()
	}
}

/// Map a listing id → slug on `/danh-sach/`.
fn listing_path(id: &str) -> &'static str {
	match id {
		"bo" => "phim-bo",
		"le" => "phim-le",
		"hoat-hinh" => "hoat-hinh",
		"chieu-rap" => "phim-chieu-rap",
		"sap-chieu" => "sap-chieu",
		"tv-shows" => "tv-shows",
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

pub struct OphimSource;

impl OphimSource {
	fn base(&self) -> String {
		match defaults_get::<String>(SETTING_BASE_URL) {
			Some(s) if !s.trim().is_empty() => s.trim().to_string(),
			_ => String::from(DEFAULT_BASE),
		}
	}
}

impl Source for OphimSource {
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
		// recently-updated list with the same filter set (OPhim supports
		// category, country, year, sort on both endpoints).
		let url = if qs.contains("keyword=") {
			format!("{base}/tim-kiem?{qs}")
		} else {
			format!("{base}/danh-sach/phim-moi-cap-nhat?{qs}")
		};
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

		// Look in the requested server first; if that server lacks the episode
		// (e.g. the app auto-resolves the first server for an episode that only
		// exists on another one) fall back to every group.
		let entry = movie
			.episodes
			.iter()
			.find(|g| g.server_name.eq_ignore_ascii_case(&stream.key))
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

		let Some(url) = absolutize_opt(entry.link_m3u8.as_deref().or(entry.link_embed.as_deref()))
		else {
			bail!(
				"Không tìm thấy liên kết phát cho tập {}.",
				episode.episode_number
			);
		};

		let mut headers = komorei::HashMap::new();
		headers.insert(String::from("Referer"), format!("{base}/"));

		let stream_type = detect_stream_type(&url);

		Ok(StreamData {
			url,
			stream_type,
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

impl ListingProvider for OphimSource {
	fn get_anime_list(&self, listing: Listing, page: i32) -> Result<AnimePageResult> {
		if !matches!(listing.kind, ListingKind::List) {
			return Ok(AnimePageResult::default());
		}
		let base = self.base();
		let path = listing_path(&listing.id);
		let (items, has_next) = fetch_page(&format!("{base}/danh-sach/{path}?page={page}"))?;
		Ok(AnimePageResult {
			entries: items.iter().map(|m| build_lite(m, &base)).collect(),
			has_next_page: has_next,
		})
	}
}

impl DynamicListings for OphimSource {
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
				id: String::from("hoat-hinh"),
				name: String::from("Hoạt Hình"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("chieu-rap"),
				name: String::from("Phim Chiếu Rạp"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("sap-chieu"),
				name: String::from("Sắp Chiếu"),
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

impl Home for OphimSource {
	fn get_home(&self) -> Result<HomeLayout> {
		let base = self.base();
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

		let mut latest: Vec<OphimMovie> = Vec::new();
		let mut bo: Vec<OphimMovie> = Vec::new();
		let mut le: Vec<OphimMovie> = Vec::new();
		for (i, resp) in responses.into_iter().enumerate() {
			let Ok(resp) = resp else { continue };
			let Ok(env) = resp.get_json_owned::<ListEnvelope>() else {
				continue;
			};
			let items = env
				.data
				.and_then(|d| d.items)
				.or(env.items)
				.unwrap_or_default();
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
					image_url: absolutize_opt(m.poster_url.as_deref().or(m.thumb_url.as_deref())),
					value: Some(LinkValue::Anime(build_lite(m, &base))),
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
					image_url: absolutize_opt(m.poster_url.as_deref().or(m.thumb_url.as_deref())),
					value: Some(LinkValue::Anime(build_lite(m, &base))),
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
			("hoat-hinh", "Hoạt Hình"),
			("chieu-rap", "Phim Chiếu Rạp"),
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

impl DynamicFilters for OphimSource {
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
				"Nội dung khai thác từ API OPhim (https://phimapi.com). Có thể đổi base URL ở Cài đặt nguồn.",
			),
		])
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Settings
// ────────────────────────────────────────────────────────────────────────────

impl DynamicSettings for OphimSource {
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		Ok(vec![
			TextSetting {
				key: SETTING_BASE_URL.into(),
				title: "Địa chỉ API OPhim".into(),
				placeholder: Some("https://phimapi.com".into()),
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

impl NotificationHandler for OphimSource {
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

impl DeepLinkHandler for OphimSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		if let Some(idx) = url.find("/xem-phim/") {
			let rest = &url[idx + "/xem-phim/".len()..];
			let mut parts = rest.splitn(2, '/');
			if let (Some(slug), Some(ep)) = (parts.next(), parts.next())
				&& !slug.is_empty()
				&& !ep.is_empty()
			{
				return Ok(Some(DeepLinkResult::Episode {
					anime_key: slug.to_string(),
					key: ep.to_string(),
				}));
			}
			return Ok(None);
		}
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
// Migration — OPhim does not change ids across versions, keep identity.
// ────────────────────────────────────────────────────────────────────────────

impl MigrationHandler for OphimSource {
	fn handle_anime_migration(&self, key: String) -> Result<String> {
		Ok(key)
	}

	fn handle_episode_migration(&self, _anime_key: String, episode_key: String) -> Result<String> {
		Ok(episode_key)
	}
}

register_source!(
	OphimSource,
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
	fn parses_isodate_millis_utc() {
		assert_eq!(
			parse_isodate_millis("2026-09-21T18:52:19.000Z"),
			Some(1_790_016_739_000)
		);
		assert_eq!(
			parse_isodate_millis("2026-09-20T00:00:00.000Z"),
			Some(1_789_862_400_000)
		);
		assert_eq!(parse_isodate_millis("2026-09-21"), None); // missing time part
		assert_eq!(parse_isodate_millis("not-a-date"), None);
	}

	#[komorei_test]
	fn splits_season_key_into_slug_and_server() {
		assert_eq!(
			split_server("nguoi-nhan|OPhim"),
			("nguoi-nhan", Some("OPhim"))
		);
		assert_eq!(
			split_server("nguoi-nhan|Trailer"),
			("nguoi-nhan", Some("Trailer"))
		);
		assert_eq!(split_server("nguoi-nhan"), ("nguoi-nhan", None));
	}

	#[komorei_test]
	fn parses_leading_digits_of_episode_labels() {
		assert_eq!(parse_episode_number("Tập 12/24"), "12");
		assert_eq!(parse_episode_number("Full"), "1");
		assert_eq!(parse_episode_number("Tập 1179"), "1179");
		assert_eq!(parse_first_int(Some("Tập 2/24")), Some(2));
		assert_eq!(parse_first_int(Some("Full")), None);
	}

	#[komorei_test]
	fn episode_total_tolerates_the_string_to_integer_drift() {
		#[derive(Deserialize)]
		struct Msg {
			#[serde(default, deserialize_with = "de_opt_str_or_num")]
			episode_total: Option<String>,
		}
		// Newer phimapi forks: raw integer.
		let m: Msg = serde_json::from_str(r#"{"episode_total": 25}"#).unwrap();
		assert_eq!(m.episode_total.as_deref(), Some("25"));
		// Classic envelope: string form.
		let m: Msg = serde_json::from_str(r#"{"episode_total": "25"}"#).unwrap();
		assert_eq!(m.episode_total.as_deref(), Some("25"));
		// Labels still parse (`parse_first_int` semantics after normalization).
		let m: Msg = serde_json::from_str(r#"{"episode_total": "Tập 12/24"}"#).unwrap();
		assert_eq!(m.episode_total.as_deref(), Some("Tập 12/24"));
		// Absent → default None.
		let m: Msg = serde_json::from_str(r#"{}"#).unwrap();
		assert_eq!(m.episode_total, None);
	}

	#[komorei_test]
	fn absolutizes_protocol_relative_images() {
		assert_eq!(
			absolutize_url("//img.ophim.com/a.jpg"),
			"https://img.ophim.com/a.jpg"
		);
		assert_eq!(
			absolutize_url("https://img.ophim.com/a.jpg"),
			"https://img.ophim.com/a.jpg"
		);
		assert_eq!(absolutize_opt(None), None);
	}

	#[komorei_test]
	fn detects_stream_type_from_url() {
		assert_eq!(
			detect_stream_type("https://cdn.test/a.m3u8"),
			StreamType::HLS
		);
		assert_eq!(
			detect_stream_type("https://cdn.test/hls/x.m3u8"),
			StreamType::HLS
		);
		assert_eq!(
			detect_stream_type("https://cdn.test/a.mp4"),
			StreamType::MP4
		);
		assert_eq!(
			detect_stream_type("https://cdn.test/a.mpd"),
			StreamType::DASH
		);
		assert_eq!(
			detect_stream_type("https://cdn.test/embed?id=1"),
			StreamType::OTHER
		);
	}

	#[komorei_test]
	fn strips_html_tags_and_entities_from_content() {
		assert_eq!(
			strip_html("<p>Người Nhện chiến đấu&hellip;</p>&nbsp;"),
			"Người Nhện chiến đấu... "
		);
		assert_eq!(strip_html("plain text"), "plain text");
	}

	#[komorei_test]
	fn maps_genre_and_country_names_to_ophim_slugs() {
		assert_eq!(genre_slug("Hành Động"), "hanh-dong");
		assert_eq!(genre_slug("Mecha"), "mecha");
		assert_eq!(genre_slug("Sci-Fi"), "sci-fi"); // not in the table → ASCII slugify
		assert_eq!(country_slug("Mỹ"), "my");
		assert_eq!(country_slug("Nhật Bản"), "nhat-ban");
	}

	#[komorei_test]
	fn maps_ophim_status_enums() {
		let completed = OphimMovie {
			status: Some(String::from("completed")),
			..Default::default()
		};
		let ongoing = OphimMovie {
			status: Some(String::from("ongoing")),
			..Default::default()
		};
		assert_eq!(map_status(&completed), AnimeStatus::Completed);
		assert_eq!(map_status(&ongoing), AnimeStatus::Ongoing);
		assert_eq!(map_status(&OphimMovie::default()), AnimeStatus::Unknown);
	}

	#[komorei_test]
	fn season_key_roundtrip_builds_linked_anime_ids() {
		// fake detail JSON: the main server first (like the real OPhim)
		let groups = vec![
			EpisodeGroup {
				server_name: String::from("OPhim"),
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
			"https://ophim.example",
			&OphimMovie {
				name: String::from("Người Nhện"),
				episodes: groups.clone(),
				..Default::default()
			},
			"nguoi-nhan",
		);
		// the largest server is sorted first
		assert_eq!(
			full.seasons
				.iter()
				.map(|s| s.anime_id.as_str())
				.collect::<Vec<_>>(),
			vec!["nguoi-nhan|OPhim", "nguoi-nhan|Trailer"]
		);
		// the season key returns the episodes of that server
		assert_eq!(
			episodes_for_server(&groups, Some("OPhim"))
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
	fn reads_root_level_episodes_from_phimapi_detail() {
		// phimapi.com `/phim/{slug}` → `{ status, msg, movie, episodes }` with
		// the groups at the ROOT of the envelope.
		let json = r##"{
			"status": true,
			"msg": "done",
			"movie": { "name": "33 Người Thợ Mỏ", "slug": "33-nguoi-tho-mo" },
			"episodes": [
				{ "server_name": "Vietsub", "is_ai": false, "server_data": [
					{ "name": "Full", "slug": "full", "filename": "The 33 2015",
					  "link_embed": "https://player.phimapi.com/player/?url=...",
					  "link_m3u8": "https://a.kvp726.com/20260921/oqKrNtja/index.m3u8" }
				] }
			]
		}"##;
		let env: DetailEnvelope = serde_json::from_str(json).expect("detail envelope decodes");
		assert!(env.movie.is_some());
		let root_eps = env.episodes.clone().unwrap_or_default();
		assert_eq!(root_eps.len(), 1);
		assert_eq!(root_eps[0].server_name, "Vietsub");
		assert_eq!(root_eps[0].server_data[0].slug, "full");
		assert!(root_eps[0].server_data[0].link_m3u8.is_some());

		// fetch_detail merge: root-level episodes win when the movie has none.
		let movie = OphimMovie {
			episodes: Vec::new(),
			..Default::default()
		};
		let mut env = env;
		env.movie = Some(movie);
		let mut full = env.movie.take().unwrap();
		if full.episodes.is_empty() {
			full.episodes = env.episodes.unwrap_or_default();
		}
		assert_eq!(full.episodes.len(), 1);
		assert_eq!(full.episodes[0].server_name, "Vietsub");
	}

	#[komorei_test]
	fn reads_root_pagination_from_phimapi_list() {
		// phimapi.com `/danh-sach/{path}?page=` → flat `{ status, msg, items,
		// pagination }` (pagination at the ROOT, not under data.params).
		let json = r##"{
			"status": true,
			"msg": "done",
			"items": [
				{ "name": "A", "slug": "a", "poster_url": "https://phimimg.com/a.jpg", "year": 2024 }
			],
			"pagination": { "totalItems": 30106, "totalItemsPerPage": 24, "currentPage": 1, "totalPages": 1255 }
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

		// classic OPhim still works: pagination nested under data.params.
		let json = r##"{
			"data": {
				"items": [ { "name": "B", "slug": "b" } ],
				"params": { "pagination": { "currentPage": 2, "totalPages": 2 } }
			}
		}"##;
		let env: ListEnvelope = serde_json::from_str(json).expect("classic envelope decodes");
		let nested = env
			.data
			.as_ref()
			.and_then(|d| d.params.as_ref())
			.and_then(|p| p.pagination.as_ref());
		assert_eq!(nested.map(|pg| pg.current_page), Some(2));
		assert!(!nested.map(|pg| pg.current_page < pg.total_pages).unwrap_or(false));
	}

	#[komorei_test]
	fn reads_search_items_under_data() {
		// phimapi `/tim-kiem` → `{ status, message, data: { items, params } }`.
		let json = r##"{
			"status": "success",
			"message": "done",
			"data": {
				"items": [ { "name": "Naruto", "slug": "naruto", "year": 2002 } ],
				"params": { "type": "series" }
			}
		}"##;
		let env: ListEnvelope = serde_json::from_str(json).expect("search envelope decodes");
		let items = env
			.data
			.and_then(|d| d.items)
			.unwrap_or_default();
		assert_eq!(items.len(), 1);
		assert_eq!(items[0].name, "Naruto");
	}
}
