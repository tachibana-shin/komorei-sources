//! # KKPhim source (`vi.kkphim`)
//!
//! Scrapes the KKPhim "Phim Nguồn" website (kkphim.com — the brand that keeps
//! rotating domains: `kkphim.vip`, `kkphim1.com`, ...). KKPhim is a frontend
//! over the same phimapi data family, but the movie pages embed playable HLS
//! links directly (their detail page even links to the phimapi v1 API as its
//! "Nguồn" button).
//!
//! Because the site changes domains (kkphim.com → kkphim.vip → kkphim1.com),
//! the base URL can be changed in Source settings (`base_url`), defaulting to
//! the currently-live `https://kkphim1.com`.
//!
//! ## Endpoints
//!
//! - **Browse** — `{base}/danh-sach/{phim-bo,phim-le,hoat-hinh,phim-chieu-rap,
//!   tv-shows,phim-moi,phim-bo-dang-chieu,phim-bo-hoan-thanh}?page=N`,
//!   `{base}/the-loai/{slug}?page=N`, `{base}/quoc-gia/{slug}?page=N`. All of
//!   these render a `<table class="table data-table">` — one `<tr>` per movie.
//! - **Search** — `{base}/tim-kiem-nhanh?keyword=&page=` → JSON
//!   `{ total, current_page, last_page, items: [{ name, origin_name, year,
//!   quality, episode_current, vote, poster, url: "/phim/{slug}" }] }`.
//! - **Detail** — `{base}/phim/{slug}` → HTML. The metadata lives in
//!   `.detail-head` (`.head-title`, `.head-origin`, `.detail-poster img`,
//!   `.head-tags .tag`, `.meta-card` blocks, "Nội dung phim" description) and
//!   the episode/stream data is a JSON array embedded in
//!   `<script type="application/json" id="srcData">`:
//!   `[{ "server_name", "is_ai", "server_data": [{ "name", "slug",
//!   "filename", "link_embed", "link_m3u8" }] }]`. `link_m3u8` is a direct
//!   HLS url on the kkphim CDN (`a.kvp726.com`, `v7.kkphimplayer7.com`, ...).
//!
//! ## Data model mapping to Komorei
//!
//! - **Season = one playback server** (like vi.ophim). `AnimeSeason.anime_id`
//!   is encoded as `"{slug}|{server_name}"`; `get_anime_update`/`get_stream`
//!   strip the part after `|` to select the group. Servers are sorted by
//!   episode count descending, so the largest one is the default.
//! - **Episode key = `slug` of the entry** in `server_data` (e.g. `tap-01`).
//! - `get_stream_list`: each server is a `StreamInfo`. `get_stream`: refetches
//!   the detail, finds the group by server and the entry by episode key, and
//!   **falls back to the first group containing the episode** when the
//!   requested server lacks it.

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
	imports::html::{Document, Element},
	imports::net::Request,
	prelude::*,
	serde::Deserialize,
};

const SOURCE_ID: &str = "vi.kkphim";
// The KKPhim brand rotates domains (kkphim.com, kkphim.vip, kkphim1.com, ...).
// kkphim.com is currently dead (connection refused) — point the default at the
// live site and let the user migrate via the `base_url` setting when it moves.
const DEFAULT_BASE: &str = "https://kkphim1.com";
const SETTING_BASE_URL: &str = "base_url";
const SETTING_LAST_NOTIFICATION: &str = "last_notification";

// ────────────────────────────────────────────────────────────────────────────
// Genre & Country catalogs (name → KKPhim slug), mirroring the site's own
// navigation lists (`/the-loai/*`, `/quoc-gia/*`). Used for the home "Thể
// Loại" chips and the MultiSelect filters.
// ────────────────────────────────────────────────────────────────────────────

const GENRES: &[(&str, &str)] = &[
	("Bí Ẩn", "bi-an"),
	("Chiến Tranh", "chien-tranh"),
	("Chính Kịch", "chinh-kich"),
	("Cổ Trang", "co-trang"),
	("Gia Đình", "gia-dinh"),
	("Hài Hước", "hai-huoc"),
	("Hành Động", "hanh-dong"),
	("Hình Sự", "hinh-su"),
	("Học Đường", "hoc-duong"),
	("Khoa Học", "khoa-hoc"),
	("Kinh Dị", "kinh-di"),
	("Kinh Điển", "kinh-dien"),
	("Lịch Sử", "lich-su"),
	("Miền Tây", "mien-tay"),
	("Phim 18+", "phim-18"),
	("Phim Ngắn", "phim-ngan"),
	("Phiêu Lưu", "phieu-luu"),
	("Thần Thoại", "than-thoai"),
	("Thể Thao", "the-thao"),
	("Trẻ Em", "tre-em"),
	("Tài Liệu", "tai-lieu"),
	("Tâm Lý", "tam-ly"),
	("Tình Cảm", "tinh-cam"),
	("Viễn Tưởng", "vien-tuong"),
	("Võ Thuật", "vo-thuat"),
	("Âm Nhạc", "am-nhac"),
];

const COUNTRIES: &[(&str, &str)] = &[
	("Hàn Quốc", "han-quoc"),
	("Nhật Bản", "nhat-ban"),
	("Trung Quốc", "trung-quoc"),
	("Hồng Kông", "hong-kong"),
	("Đài Loan", "dai-loan"),
	("Thái Lan", "thai-lan"),
	("Ấn Độ", "an-do"),
	("Âu Mỹ", "au-my"),
	("Anh", "anh"),
	("Pháp", "phap"),
	("Đức", "duc"),
	("Tây Ban Nha", "tay-ban-nha"),
	("Ý", "y"),
	("Nga", "nga"),
	("Ukraina", "ukraina"),
	("Canada", "canada"),
	("Úc", "uc"),
	("Mỹ", "my"),
	("Brazil", "brazil"),
	("Mexico", "mexico"),
	("Indonesia", "indonesia"),
	("Malaysia", "malaysia"),
	("Philippines", "philippines"),
	("Singapore", "singapore"),
	("Na Uy", "na-uy"),
	("Thụy Điển", "thuy-dien"),
	("Đan Mạch", "dan-mach"),
	("Hà Lan", "ha-lan"),
	("Bồ Đào Nha", "bo-dao-nha"),
	("Ba Lan", "ba-lan"),
	("Hy Lạp", "hy-lap"),
	("Thổ Nhĩ Kỳ", "tho-nhi-ky"),
	("Việt Nam", "viet-nam"),
];

// ────────────────────────────────────────────────────────────────────────────
// JSON models
// ────────────────────────────────────────────────────────────────────────────

/// One row of the `/tim-kiem-nhanh` search JSON.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct SearchItem {
	name: String,
	origin_name: String,
	year: Option<i32>,
	quality: Option<String>,
	episode_current: Option<String>,
	vote: Option<f32>,
	poster: Option<String>,
	/// `/phim/{slug}`
	url: Option<String>,
}

impl SearchItem {
	/// Extract the slug from `url` (`/phim/foo-bar` → `foo-bar`).
	fn slug(&self) -> String {
		slug_from_path(self.url.as_deref().unwrap_or("")).unwrap_or_default()
	}
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct SearchEnvelope {
	total: Option<i32>,
	current_page: Option<i32>,
	last_page: Option<i32>,
	items: Vec<SearchItem>,
}

/// One entry of the detail page's `srcData` JSON (`server_data`).
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct EpEntry {
	name: String,
	slug: String,
	filename: Option<String>,
	link_embed: Option<String>,
	link_m3u8: Option<String>,
}

/// One `server_name` group of the `srcData` JSON.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct EpGroup {
	server_name: String,
	server_data: Vec<EpEntry>,
}

/// Everything the source needs from a detail page.
#[derive(Default)]
struct DetailInfo {
	title: String,
	origin: String,
	poster: Option<String>,
	year: Option<i32>,
	lang: Option<String>,
	quality: Option<String>,
	status: Option<String>,
	current_episode: Option<String>,
	description: Option<String>,
	rating: Option<f32>,
	rating_count: Option<i32>,
	genres: Vec<String>,
	countries: Vec<String>,
	episode_groups: Vec<EpGroup>,
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

/// Turn a protocol-relative (`//cdn...`) or root-absolute (`/path`) URL into an
/// absolute URL against `base`.
fn absolutize_url(url: &str, base: &str) -> String {
	if url.starts_with("//") {
		format!("https:{url}")
	} else if url.starts_with('/') {
		format!("{base}{url}")
	} else {
		url.to_string()
	}
}

fn absolutize_opt(url: Option<&str>, base: &str) -> Option<String> {
	url.map(|u| absolutize_url(u, base)).filter(|s| !s.is_empty())
}

/// Last path segment of an internal link: `/the-loai/hanh-dong`, `/phim/foo`
/// or `/phim/foo/` → `hanh-dong`, `foo`.
fn slug_from_path(path: &str) -> Option<String> {
	let trimmed = path.trim_end_matches('/');
	let idx = trimmed.rfind('/')?;
	let slug = &trimmed[idx + 1..];
	if slug.is_empty() {
		None
	} else {
		Some(slug.to_string())
	}
}

/// Extract the first run of digits ("Tập 12/24" → "12", "Full" → "1").
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

/// Extract the first leading float ("★ 6.4" → 6.4, "6" → 6).
fn parse_float(input: &str) -> Option<f32> {
	let s = input.trim();
	let mut end = 0;
	let mut dots = 0;
	for (i, c) in s.chars().enumerate() {
		if c.is_ascii_digit() {
			end = i + 1;
		} else if c == '.' && dots == 0 {
			dots += 1;
			end = i + 1;
		} else {
			break;
		}
	}
	if end == 0 {
		return None;
	}
	s[..end].parse().ok()
}

/// Extract the first integer inside parentheses: `/10 (1014)` → 1014.
fn parse_paren_int(input: &str) -> Option<i32> {
	let bytes = input.as_bytes();
	let mut i = 0;
	while i < bytes.len() {
		if bytes[i] == b'(' {
			let mut j = i + 1;
			while j < bytes.len() && bytes[j].is_ascii_digit() {
				j += 1;
			}
			if j > i + 1 {
				return input[i + 1..j].parse().ok();
			}
		}
		i += 1;
	}
	None
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

/// Parse `"2026-09-21T18:52:19.000Z"` → epoch millis (the browse table's
/// "Cập Nhật" column is a full ISO timestamp).
fn parse_isodate_millis(input: &str) -> Option<i64> {
	let input = input.trim();
	let bytes = input.as_bytes();
	if bytes.len() < 19 {
		return None;
	}
	let d = |i: usize| -> i64 { (bytes[i] - b'0') as i64 };
	Some(
		days_from_civil(
			d(0) * 1000 + d(1) * 100 + d(2) * 10 + d(3),
			d(5) * 10 + d(6),
			d(8) * 10 + d(9),
		) * 86_400_000
			+ (d(11) * 10 + d(12)) * 3_600_000
			+ (d(14) * 10 + d(15)) * 60_000
			+ (d(17) * 10 + d(18)) * 1_000,
	)
}

/// Parse the `page` query param out of a pagination href.
fn page_param(href: &str) -> Option<i32> {
	let q = href.split('?').nth(1)?;
	for pair in q.split('&') {
		let mut it = pair.splitn(2, '=');
		if it.next() == Some("page") {
			return it.next().and_then(|v| v.trim().parse().ok());
		}
	}
	None
}

/// Map a list-row status text to [AnimeStatus]: "Tập N" → ongoing, "Hoàn Tất"
/// / "Full" → completed.
fn map_row_status(text: &str) -> AnimeStatus {
	let t = text.trim();
	if t.is_empty() {
		return AnimeStatus::Unknown;
	}
	if t.contains("Tập") && !t.contains("Hoàn Tất") {
		AnimeStatus::Ongoing
	} else {
		AnimeStatus::Completed
	}
}

fn map_status(status: Option<&str>) -> AnimeStatus {
	match status {
		Some("completed") => AnimeStatus::Completed,
		Some("ongoing") => AnimeStatus::Ongoing,
		_ => AnimeStatus::Unknown,
	}
}

fn name_links(items: &[String]) -> Vec<CategoryLink> {
	items
		.iter()
		.filter(|c| !c.is_empty())
		.map(|c| CategoryLink {
			name: c.clone(),
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
// HTML parsing
// ────────────────────────────────────────────────────────────────────────────

/// `{base}/` — parse the "Mới Cập Nhật" table into Lite cards.
fn parse_list_rows(doc: &Document, base: &str) -> Vec<Anime> {
	let mut out = Vec::new();
	if let Some(rows) = doc.select("table.data-table tbody tr") {
		for i in 0..rows.size() {
			if let Some(row) = rows.get(i) {
				if let Some(anime) = parse_list_row(&row, base) {
					out.push(anime);
				}
			}
		}
	}
	out
}

/// Parse one `<tr>` of the browse table into a Lite [Anime].
///
/// Columns: Thông tin | Năm | Tình Trạng | TMDB | IMDB | Định Dạng | Quốc Gia
/// | Cập Nhật.
fn parse_list_row(row: &Element, base: &str) -> Option<Anime> {
	let a = row.select_first("a.info-title")?;
	let title = a.text()?;
	let slug = a.attr("href").and_then(|h| slug_from_path(&h))?;
	if slug.is_empty() || title.trim().is_empty() {
		return None;
	}
	let origin = row.select_first(".info-origin").and_then(|e| e.text()).unwrap_or_default();
	let poster = row.select_first(".poster-wrap img").and_then(|i| i.attr("src"));
	let rating = row
		.select_first(".rating-badge")
		.and_then(|e| e.text())
		.and_then(|t| parse_float(&t));
	let year = row_aux(row, 2).and_then(|t| t.trim().parse().ok());
	let status_txt = row_aux(row, 3).unwrap_or_default();
	let type_txt = row_aux(row, 6).unwrap_or_default();
	let poster_abs = absolutize_opt(poster.as_deref(), base);

	Some(Anime {
		key: slug.clone(),
		source_id: SOURCE_ID.into(),
		title,
		original_title: origin,
		cover: poster_abs.clone().unwrap_or_default(),
		banner: poster_abs,
		description: None,
		episode_count: 0,
		current_episode: Some(status_txt.trim().to_string()),
		rating,
		rating_count: None,
		status: map_row_status(&status_txt),
		release_year: year_link(year),
		genres: Vec::new(),
		authors: Vec::new(),
		studio: None,
		season_of: None,
		countries: Vec::new(),
		is_featured: false,
		views: 0,
		next_episode_air_info: None,
		quality_tag: Some(type_txt.trim().to_string()),
		seasons: Vec::new(),
		episodes: None,
		url: Some(format!("{base}/phim/{slug}")),
	})
}

/// Text of the `n`-th `td` (1-based) in a browse table row.
fn row_aux(row: &Element, n: usize) -> Option<String> {
	row.select_first(&format!("td:nth-child({n})")).and_then(|td| td.text())
}

/// True when `ul.pagination` contains a page link further than `page`.
fn has_next_page(doc: &Document, page: i32) -> bool {
	let mut max_page = page;
	if let Some(links) = doc.select("ul.pagination a.page-link[href]") {
		for i in 0..links.size() {
			if let Some(a) = links.get(i) {
				if let Some(href) = a.attr("href")
					&& let Some(p) = page_param(&href)
				{
					max_page = max_page.max(p);
				}
			}
		}
	}
	max_page > page
}

/// Parse `{base}/phim/{slug}` into [DetailInfo].
fn fetch_detail(base: &str, slug: &str) -> Result<DetailInfo> {
	let doc = Request::get(format!("{base}/phim/{slug}"))?.html()?;
	Ok(parse_detail(&doc))
}

fn parse_detail(doc: &Document) -> DetailInfo {
	let mut d = DetailInfo::default();

	// Title / origin / poster
	d.title = first_text(doc, ".head-title").unwrap_or_default();
	d.origin = first_text(doc, ".head-origin").unwrap_or_default();
	d.poster = doc
		.select_first(".detail-poster img")
		.and_then(|e| e.attr("src"));

	// head-tags: type / year / lang / quality
	if let Some(tags) = doc.select(".head-tags .tag") {
		for i in 0..tags.size() {
			if let Some(t) = tags.get(i) {
				let cls = t.class_name().unwrap_or_default();
				let txt = t.text().unwrap_or_default();
				if cls.split(' ').any(|c| c == "tag-type") {
					let _ = txt; // tag-type is "single"/"series"/... — not on Anime
				} else if cls.split(' ').any(|c| c == "tag-year") {
					d.year = txt.trim().parse().ok();
				} else if cls.split(' ').any(|c| c == "tag-lang") {
					d.lang = Some(txt);
				} else if cls.split(' ').any(|c| c == "tag-quality") {
					d.quality = Some(txt);
				}
			}
		}
	}

	// meta-cards: Thể loại / Quốc gia / Thông tin / TMDB
	if let Some(cards) = doc.select(".meta-card") {
		for i in 0..cards.size() {
			if let Some(card) = cards.get(i) {
				parse_meta_card(&card, &mut d);
			}
		}
	}

	// "Nội dung phim" description block
	if let Some(titles) = doc.select("h5.section-title") {
		for i in 0..titles.size() {
			if let Some(t) = titles.get(i) {
				let is_desc = t
					.text()
					.map(|x| x.contains("Nội dung phim"))
					.unwrap_or(false);
				if is_desc
					&& let Some(sec) = t.parent()
				{
					d.description = sec
						.select_first(".text-light-emphasis")
						.and_then(|e| e.text());
					break;
				}
			}
		}
	}

	// srcData JSON with the episode groups
	if let Some(s) = doc.select_first("script#srcData") {
		if let Some(data) = s.data() {
			d.episode_groups = serde_json::from_str(&data).unwrap_or_default();
		}
	}

	d
}

fn parse_meta_card(card: &Element, d: &mut DetailInfo) {
	let head = card.select_first(".head").and_then(|h| h.text()).unwrap_or_default();
	if head.contains("Thể loại") {
		d.genres = chip_links(card);
	} else if head.contains("Quốc gia") {
		d.countries = chip_links(card);
	} else if head.contains("Thông tin") {
		d.status = card.select_first(".mc-badge").and_then(|b| b.text());
		if let Some(smalls) = card.select(".small") {
			for j in 0..smalls.size() {
				if let Some(s) = smalls.get(j) {
					let lbl = s
						.select_first(".text-secondary")
						.and_then(|x| x.text())
						.unwrap_or_default();
					if lbl.contains("Tập hiện tại") {
						d.current_episode = s.select_first("strong").and_then(|x| x.text());
					}
				}
			}
		}
	} else if head.contains("TMDB") {
		d.rating = card
			.select_first(".small strong")
			.and_then(|x| x.text())
			.and_then(|t| parse_float(&t));
		d.rating_count = card.text().and_then(|t| parse_paren_int(&t));
	}
}

/// `a.chip` link names (genre/country) inside a meta-card.
fn chip_links(card: &Element) -> Vec<String> {
	let mut out = Vec::new();
	if let Some(chips) = card.select("a.chip") {
		for i in 0..chips.size() {
			if let Some(c) = chips.get(i) {
				let name = c.text().unwrap_or_default();
				if !name.trim().is_empty() {
					out.push(name);
				}
			}
		}
	}
	out
}

fn first_text(doc: &Document, selector: &str) -> Option<String> {
	doc.select_first(selector).and_then(|e| e.text())
}

// ────────────────────────────────────────────────────────────────────────────
// Anime construction
// ────────────────────────────────────────────────────────────────────────────

fn lite_from_search(item: &SearchItem, base: &str) -> Anime {
	let slug = if item.url.is_some() {
		item.slug()
	} else {
		slugify(&item.name)
	};
	let poster = absolutize_opt(item.poster.as_deref(), base);
	Anime {
		key: slug.clone(),
		source_id: SOURCE_ID.into(),
		title: item.name.clone(),
		original_title: item.origin_name.clone(),
		cover: poster.clone().unwrap_or_default(),
		banner: poster,
		description: None,
		episode_count: 0,
		current_episode: item.episode_current.clone(),
		rating: item.vote,
		rating_count: None,
		status: AnimeStatus::Unknown,
		release_year: year_link(item.year),
		genres: Vec::new(),
		authors: Vec::new(),
		studio: None,
		season_of: None,
		countries: Vec::new(),
		is_featured: false,
		views: 0,
		next_episode_air_info: None,
		quality_tag: item.quality.clone(),
		seasons: Vec::new(),
		episodes: None,
		url: item
			.url
			.as_ref()
			.map(|u| absolutize_url(u, base)),
	}
}

/// Full detail: `seasons` = playback servers (sorted by episode count desc).
fn build_full(info: &DetailInfo, base: &str, slug: &str) -> Anime {
	let mut groups: Vec<&EpGroup> = info
		.episode_groups
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

	let episode_count = groups
		.first()
		.map(|g| g.server_data.len() as i32)
		.unwrap_or(0);

	Anime {
		key: slug.to_string(),
		source_id: SOURCE_ID.into(),
		title: info.title.clone(),
		original_title: info.origin.clone(),
		cover: info.poster.clone().unwrap_or_default(),
		banner: info.poster.clone(),
		description: info.description.clone(),
		episode_count,
		current_episode: info.current_episode.clone(),
		rating: info.rating,
		rating_count: info.rating_count,
		status: map_status(info.status.as_deref()),
		release_year: year_link(info.year),
		genres: name_links(&info.genres),
		authors: Vec::new(),
		studio: None,
		season_of: None,
		countries: name_links(&info.countries),
		is_featured: false,
		views: 0,
		next_episode_air_info: None,
		quality_tag: info.quality.clone(),
		seasons,
		episodes: None,
		url: Some(format!("{base}/phim/{slug}")),
	}
}

/// Episodes for the selected server (None → first server = largest).
fn episodes_for_server(groups: &[EpGroup], server: Option<&str>) -> Vec<Episode> {
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

/// Stub episode for the Home screen — the grow episode badge + update date.
fn row_home_episode(anime: &Anime, row: &Element) -> Episode {
	let current = row_aux(row, 3).unwrap_or_default();
	let date = row_aux(row, 8).and_then(|t| parse_isodate_millis(&t));
	Episode {
		key: format!("{}__latest", anime.key),
		episode_number: parse_episode_number(&current),
		title: Some(anime.title.clone()),
		date_uploaded: date,
		thumbnail: Some(anime.cover.clone()),
		quality: None,
		duration_seconds: None,
		url: None,
		language: Some(String::from("vi")),
		locked: false,
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Source
// ────────────────────────────────────────────────────────────────────────────

pub struct KkphimSource;

impl KkphimSource {
	fn base(&self) -> String {
		match defaults_get::<String>(SETTING_BASE_URL) {
			Some(s) if !s.trim().is_empty() => s.trim().to_string(),
			_ => String::from(DEFAULT_BASE),
		}
	}
}

impl Source for KkphimSource {
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

		// Keyword search → the JSON quick-search endpoint.
		if let Some(q) = query.as_ref()
			&& !q.trim().is_empty()
		{
			let mut qp = QueryParameters::with_capacity(2);
			let page_str = page.to_string();
			qp.push("keyword", Some(q));
			qp.push("page", Some(page_str.as_str()));
			let url = format!("{base}/tim-kiem-nhanh?{}", qp.to_string());
			let env: SearchEnvelope = Request::get(&url)?.json_owned()?;
			let last = env.last_page.unwrap_or(page);
			return Ok(AnimePageResult {
				entries: env.items.iter().map(|it| lite_from_search(it, &base)).collect(),
				has_next_page: page < last,
			});
		}

		// No keyword → browse the recently-updated list through the filters.
		let mut qp = QueryParameters::with_capacity(6);
		let page_str = page.to_string();
		qp.push("page", Some(page_str.as_str()));
		apply_filters(&mut qp, &filters);
		let url = format!("{base}/danh-sach/phim-moi?{}", qp.to_string());
		let doc = Request::get(&url)?.html()?;
		Ok(fetch_list_page(&doc, &base, page))
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
			let info = fetch_detail(&self.base(), slug)?;
			if needs_details {
				let full = build_full(&info, &self.base(), slug);
				anime.copy_from(full);
			}
			if needs_chapters {
				anime.episodes = Some(episodes_for_server(&info.episode_groups, server.as_deref()));
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
		let info = fetch_detail(&base, slug)?;

		// Look in the requested server first; if that server lacks the episode
		// (e.g. the app auto-resolves the first server for an episode that only
		// exists on another one) fall back to every group.
		let entry = info
			.episode_groups
			.iter()
			.find(|g| g.server_name.eq_ignore_ascii_case(&stream.key))
			.and_then(|g| g.server_data.iter().find(|e| e.slug == episode.key))
			.or_else(|| {
				info.episode_groups
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

		let Some(url) = absolutize_opt(
			entry.link_m3u8.as_deref().or(entry.link_embed.as_deref()),
			&base,
		) else {
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

impl ListingProvider for KkphimSource {
	fn get_anime_list(&self, listing: Listing, page: i32) -> Result<AnimePageResult> {
		if !matches!(listing.kind, ListingKind::List) {
			return Ok(AnimePageResult::default());
		}
		let base = self.base();
		// `listing.id` IS the path on the site ("danh-sach/phim-bo",
		// "the-loai/hanh-dong", ...).
		let url = format!("{base}/{}?page={page}", listing.id);
		let doc = Request::get(&url)?.html()?;
		Ok(fetch_list_page(&doc, &base, page))
	}
}

/// Shared tail of a list page fetch: rows + has_next.
fn fetch_list_page(doc: &Document, base: &str, page: i32) -> AnimePageResult {
	let entries = parse_list_rows(doc, base);
	let has_next_page = has_next_page(doc, page);
	AnimePageResult {
		entries,
		has_next_page,
	}
}

impl DynamicListings for KkphimSource {
	fn get_dynamic_listings(&self) -> Result<Vec<Listing>> {
		Ok(vec![
			Listing {
				id: String::from("danh-sach/phim-moi"),
				name: String::from("Phim Mới"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("danh-sach/phim-bo"),
				name: String::from("Phim Bộ"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("danh-sach/phim-le"),
				name: String::from("Phim Lẻ"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("danh-sach/phim-bo-dang-chieu"),
				name: String::from("Phim Bộ Đang Chiếu"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("danh-sach/phim-bo-hoan-thanh"),
				name: String::from("Phim Bộ Hoàn Thành"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("danh-sach/hoat-hinh"),
				name: String::from("Hoạt Hình"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("danh-sach/phim-chieu-rap"),
				name: String::from("Phim Chiếu Rạp"),
				kind: ListingKind::List,
			},
			Listing {
				id: String::from("danh-sach/tv-shows"),
				name: String::from("TV Shows"),
				kind: ListingKind::List,
			},
		])
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Home
// ────────────────────────────────────────────────────────────────────────────

impl Home for KkphimSource {
	fn get_home(&self) -> Result<HomeLayout> {
		let base = self.base();
		let doc = Request::get(format!("{base}/"))?.html()?;

		// The homepage's "Mới Cập Nhật" table is the single data source: parse
		// each row once into a (lite anime + grow-episode stub) pair.
		let mut listed: Vec<AnimeWithEpisode> = Vec::new();
		if let Some(rows) = doc.select("table.data-table tbody tr") {
			for i in 0..rows.size() {
				if let Some(row) = rows.get(i)
					&& let Some(anime) = parse_list_row(&row, &base)
				{
					let episode = row_home_episode(&anime, &row);
					listed.push(AnimeWithEpisode { anime, episode });
				}
			}
		}
		let latest: Vec<Anime> = listed.iter().map(|e| e.anime.clone()).collect();

		let mut components = Vec::new();

		// Featured (banner) — first 5 of the "Mới Cập Nhật" table.
		if !latest.is_empty() {
			let links: Vec<Link> = latest
				.iter()
				.take(5)
				.map(|a| Link {
					title: a.title.clone(),
					image_url: if a.cover.is_empty() {
						None
					} else {
						Some(a.cover.clone())
					},
					value: Some(LinkValue::Anime(a.clone())),
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

			// Recently updated (grow episode + date from the table row).
			let first_8 = listed
				.iter()
				.take(8)
				.cloned()
				.collect::<Vec<AnimeWithEpisode>>();
			components.push(HomeComponent {
				title: Some(String::from("Mới Cập Nhật")),
				value: HomeComponentValue::AnimeEpisodeList {
					page_size: None,
					entries: first_8,
					listing: Some(Listing {
						id: String::from("danh-sach/phim-moi"),
						name: String::from("Phim Mới"),
						kind: ListingKind::List,
					}),
				},
				..Default::default()
			});
		}

		// Genre chips ("Thể Loại") → search with the "category" filter.
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

		// Quick links to the sections.
		let links: Vec<Link> = [
			("danh-sach/phim-moi", "Phim Mới"),
			("danh-sach/phim-bo", "Phim Bộ"),
			("danh-sach/phim-le", "Phim Lẻ"),
			("danh-sach/phim-bo-dang-chieu", "Phim Bộ Đang Chiếu"),
			("danh-sach/phim-bo-hoan-thanh", "Phim Bộ Hoàn Thành"),
			("danh-sach/hoat-hinh", "Hoạt Hình"),
			("danh-sach/phim-chieu-rap", "Phim Chiếu Rạp"),
			("danh-sach/tv-shows", "TV Shows"),
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

/// Push applied filters into QueryParameters (`category`/`country` slugs, a
/// single-value `year`, and `sort_field`/`sort_type` — the same parameter names
/// the kkphim list pages accept).
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

impl DynamicFilters for KkphimSource {
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
				"Nội dung khai thác từ website KKPhim (https://kkphim1.com). Website hay đổi tên miền, có thể sửa base URL ở Cài đặt nguồn.",
			),
		])
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Settings
// ────────────────────────────────────────────────────────────────────────────

impl DynamicSettings for KkphimSource {
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		Ok(vec![
			TextSetting {
				key: SETTING_BASE_URL.into(),
				title: "Địa chỉ website KKPhim".into(),
				placeholder: Some("https://kkphim1.com".into()),
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

impl NotificationHandler for KkphimSource {
	fn handle_notification(&self, notification: String) {
		// No meaningful cache to clear — the base_url is read directly on each
		// request via defaults_get.
		defaults_set(
			SETTING_LAST_NOTIFICATION,
			DefaultValue::String(notification),
		);
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Deep links
// ────────────────────────────────────────────────────────────────────────────

impl DeepLinkHandler for KkphimSource {
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
// Migration — KKPhim does not change ids across versions, keep identity.
// ────────────────────────────────────────────────────────────────────────────

impl MigrationHandler for KkphimSource {
	fn handle_anime_migration(&self, key: String) -> Result<String> {
		Ok(key)
	}

	fn handle_episode_migration(&self, _anime_key: String, episode_key: String) -> Result<String> {
		Ok(episode_key)
	}
}

register_source!(
	KkphimSource,
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
// Tests — run on the host via `cargo test` (komorei-test-runner executes the
// wasm test harness; `Html::parse` uses the runner's own HTML host).
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
	use super::*;
	use komorei::imports::html::Html;
	use komorei_test::komorei_test;

	const ROW_HTML: &str = r#"<table class="table data-table align-middle mb-0">
<thead><tr>
<th>Thông tin</th><th class="col-hide-sm">Năm</th><th class="col-hide-sm">Tình Trạng</th>
<th class="col-hide-sm">TMDB</th><th class="col-hide-sm">IMDB</th><th class="col-hide-sm">Định Dạng</th>
<th class="col-hide-sm">Quốc Gia</th><th class="col-hide-sm">Cập Nhật</th>
</tr></thead>
<tbody>
<tr>
<td><div class="d-flex gap-2">
  <div class="poster-wrap">
    <img src="https://phimimg.com/upload/vod/20231113-1/bcd21adf3bfd735e07f5a2e2b9513743.jpg" alt="" loading="lazy">
    <span class="rating-badge"><i class="bi bi-star-fill"></i> 6.4</span>
  </div>
  <div class="min-w-0">
    <a href="/phim/33-nguoi-tho-mo" class="info-title">33 Người Thợ Mỏ</a>
    <div class="info-origin">The 33</div>
    <div class="info-status-sm"><span class="st-badge st-done">Full</span></div>
  </div>
</div></td>
<td class="text-secondary col-hide-sm">2015</td>
<td class="col-hide-sm"><span class="st-badge st-done">Full</span></td>
<td class="col-hide-sm"><a href="https://www.themoviedb.org/movie/293646" class="src-badge src-tmdb">movie-293646</a></td>
<td class="col-hide-sm"><a href="https://www.imdb.com/title/tt2006295/" class="src-badge src-imdb">tt2006295</a></td>
<td class="text-secondary text-nowrap col-hide-sm">Phim Lẻ</td>
<td class="text-nowrap col-hide-sm"><img src="/uploads/flag/us.svg" alt="Âu Mỹ" title="Âu Mỹ" class="flag-icon"></td>
<td class="small text-nowrap col-hide-sm text-secondary"><i class="bi bi-calendar3"></i> 2026-09-21T21:55:35.000Z</td>
</tr>
<tr>
<td><div class="d-flex gap-2">
  <div class="min-w-0">
    <a href="/phim/dieu-con-thieu" class="info-title">Điều Còn Thiếu</a>
    <div class="info-origin">The Missing Piece</div>
  </div>
</div></td>
<td class="text-secondary col-hide-sm">2024</td>
<td class="col-hide-sm"><span class="st-badge st-live">Tập 6</span></td>
<td class="col-hide-sm"></td>
<td class="col-hide-sm"></td>
<td class="text-secondary text-nowrap col-hide-sm">Phim Bộ</td>
<td class="text-nowrap col-hide-sm"></td>
<td class="small text-nowrap col-hide-sm text-secondary"><i class="bi bi-calendar3"></i> 2026-09-22T04:12:00.000Z</td>
</tr>
</tbody></table>"#;

	const SRC_DATA_JSON: &str = r#"[{"server_name":"Youtube","is_ai":false,"server_data":[
		{"name":"Tập 1","slug":"tap-1","filename":"","link_embed":"https://player.phimapi.com/player/?url=a","link_m3u8":"https://a.kvp726.com/x/index.m3u8"},
		{"name":"Tập 2","slug":"tap-2","filename":"","link_embed":"","link_m3u8":"https://a.kvp726.com/y/index.m3u8"},
		{"name":"Tập 3","slug":"tap-3","filename":"","link_embed":"","link_m3u8":"https://a.kvp726.com/z/index.m3u8"}]},
		{"server_name":"Vietsub","is_ai":false,"server_data":[
		{"name":"Full","slug":"full","filename":"The 33","link_embed":"https://player.phimapi.com/player/?url=https://a.kvp726.com/20260921/oqKrNtja/index.m3u8","link_m3u8":"https://a.kvp726.com/20260921/oqKrNtja/index.m3u8"}]}]"#;

	const DETAIL_HTML: &str = r#"<div>
<div class="detail-head">
  <div class="detail-poster"><img src="https://phimimg.com/upload/vod/20231113-1/bcd21adf3bfd735e07f5a2e2b9513743.jpg" alt=""></div>
  <div class="head-meta">
    <h2 class="head-title fw-bold mb-1">33 Người Thợ Mỏ</h2>
    <div class="head-origin text-secondary mb-2">The 33</div>
    <div class="head-tags d-flex flex-wrap gap-2 mb-3">
      <span class="tag tag-type">single</span>
      <span class="tag tag-year">2015</span>
      <span class="tag tag-lang">Vietsub</span>
      <span class="tag tag-quality">HD</span>
    </div>
  </div>
</div>
<div class="d-flex flex-wrap mb-2">
  <div class="meta-card">
    <div class="head"><span>Thể loại</span><span class="mc-badge mc-count">2</span></div>
    <a href="/the-loai/chinh-kich" class="chip">Chính Kịch</a><a href="/the-loai/lich-su" class="chip">Lịch Sử</a>
  </div>
  <div class="meta-card">
    <div class="head"><span>Quốc gia</span><span class="mc-badge mc-count">1</span></div>
    <a href="/quoc-gia/au-my" class="chip">Âu Mỹ</a>
  </div>
  <div class="meta-card">
    <div class="head"><span>Thông tin</span><span class="mc-badge mc-done">completed</span></div>
    <div class="small d-flex justify-content-between"><span class="text-secondary">Thời lượng:</span> <strong>120 phút</strong></div>
    <div class="small d-flex justify-content-between"><span class="text-secondary">Tập hiện tại:</span> <strong>Full</strong></div>
  </div>
  <div class="meta-card">
    <div class="head"><span>TMDB</span><span class="mc-badge mc-movie">movie</span></div>
    <div class="small"><span class="text-secondary">ID:</span> 293646</div>
    <div class="small"><strong>6.4</strong> <span class="text-secondary">/10 (1014)</span></div>
  </div>
</div>
<section class="mb-4">
  <h5 class="section-title mb-3">Nội dung phim</h5>
  <p class="text-secondary small"><strong>Tên khác:</strong> The 33</p>
  <div class="text-light-emphasis">Phim 33 Người Thợ Mỏ - The 33: dựa trên sự kiện có thật, 33 người thợ mỏ mắc kẹt trong hầm mỏ 69 ngày.</div>
</section>
<script type="application/json" id="srcData">###SRC_DATA_JSON###</script>
</div>"#;

	// ── Pure helpers ──────────────────────────────────────────────────────

	#[komorei_test]
	fn parses_isodate_millis_utc() {
		assert_eq!(
			parse_isodate_millis("2026-09-21T18:52:19.000Z"),
			Some(1_790_016_739_000)
		);
		// The live table wraps the ISO string with an icon node — leading space.
		assert_eq!(
			parse_isodate_millis(" 2026-09-21T21:55:35.000Z "),
			Some(1_790_027_735_000)
		);
		assert_eq!(parse_isodate_millis("2026-09-21"), None);
		assert_eq!(parse_isodate_millis("not-a-date"), None);
	}

	#[komorei_test]
	fn splits_season_key_into_slug_and_server() {
		assert_eq!(
			split_server("33-nguoi-tho-mo|Youtube"),
			("33-nguoi-tho-mo", Some("Youtube"))
		);
		assert_eq!(split_server("33-nguoi-tho-mo"), ("33-nguoi-tho-mo", None));
	}

	#[komorei_test]
	fn parses_episode_numbers_and_floats() {
		assert_eq!(parse_episode_number("Tập 12/24"), "12");
		assert_eq!(parse_episode_number("Full"), "1");
		assert_eq!(parse_float("6.4"), Some(6.4));
		assert_eq!(parse_float(" 9 "), Some(9.0));
		assert_eq!(parse_float("n/a"), None);
		assert_eq!(parse_paren_int("/10 (1014)"), Some(1014));
		assert_eq!(parse_paren_int("(42)"), Some(42));
		assert_eq!(parse_paren_int("no parens"), None);
	}

	#[komorei_test]
	fn maps_row_status_text() {
		assert_eq!(map_row_status("Full"), AnimeStatus::Completed);
		assert_eq!(map_row_status("Hoàn Tất (25/25)"), AnimeStatus::Completed);
		assert_eq!(map_row_status("Tập 9"), AnimeStatus::Ongoing);
		assert_eq!(map_row_status(""), AnimeStatus::Unknown);
		assert_eq!(map_status(Some("completed")), AnimeStatus::Completed);
		assert_eq!(map_status(Some("ongoing")), AnimeStatus::Ongoing);
		assert_eq!(map_status(None), AnimeStatus::Unknown);
	}

	#[komorei_test]
	fn extracts_slugs_and_page_params() {
		assert_eq!(
			slug_from_path("/phim/33-nguoi-tho-mo"),
			Some("33-nguoi-tho-mo".into())
		);
		assert_eq!(slug_from_path("/the-loai/hanh-dong"), Some("hanh-dong".into()));
		assert_eq!(slug_from_path("/phim/33-nguoi-tho-mo/"), Some("33-nguoi-tho-mo".into()));
		assert_eq!(slug_from_path("https://kkphim.com/x"), Some("x".into()));
		assert_eq!(slug_from_path("/"), None);
		assert_eq!(page_param("/danh-sach/phim-bo?page=2"), Some(2));
		assert_eq!(page_param("/danh-sach/phim-bo?page=2&sort=year"), Some(2));
		assert_eq!(page_param("/danh-sach/phim-bo"), None);
	}

	#[komorei_test]
	fn resolves_genre_and_country_slugs() {
		assert_eq!(genre_slug("Hành Động"), "hanh-dong");
		assert_eq!(genre_slug("Phim 18+"), "phim-18");
		assert_eq!(country_slug("Hàn Quốc"), "han-quoc");
		assert_eq!(country_slug("Âu Mỹ"), "au-my");
		// Unknown → lowercase ASCII fallback.
		assert_eq!(genre_slug("ROBOT"), "robot");
	}

	#[komorei_test]
	fn classifies_stream_urls() {
		assert_eq!(detect_stream_type("https://a.kvp726.com/x/index.m3u8"), StreamType::HLS);
		assert_eq!(detect_stream_type("https://cdn/foo.M3U8"), StreamType::HLS);
		assert_eq!(detect_stream_type("https://cdn/foo.mp4"), StreamType::MP4);
		assert_eq!(detect_stream_type("https://cdn/foo.mpd"), StreamType::DASH);
		assert_eq!(detect_stream_type("https://cdn/foo.jpg"), StreamType::OTHER);
	}

	#[komorei_test]
	fn parses_srcdata_json_into_groups() {
		let groups: Vec<EpGroup> = serde_json::from_str(SRC_DATA_JSON).unwrap();
		assert_eq!(groups.len(), 2);
		assert_eq!(groups[0].server_name, "Youtube");
		assert_eq!(groups[0].server_data.len(), 3);
		assert_eq!(groups[1].server_name, "Vietsub");
		let first = &groups[1].server_data[0];
		assert_eq!(first.slug, "full");
		assert_eq!(
			first.link_m3u8.as_deref(),
			Some("https://a.kvp726.com/20260921/oqKrNtja/index.m3u8")
		);
	}

	#[komorei_test]
	fn search_envelope_defaults_missing_fields() {
		let env: SearchEnvelope = serde_json::from_str(
			r#"{ "total": 1, "items": [{ "name": "Điều Còn Thiếu", "url": "/phim/dieu-con-thieu" }] }"#,
		)
		.unwrap();
		assert_eq!(env.total, Some(1));
		assert_eq!(env.last_page, None);
		assert_eq!(env.items.len(), 1);
		assert_eq!(env.items[0].slug(), "dieu-con-thieu");
		assert_eq!(env.items[0].vote, None);
	}

	#[komorei_test]
	fn episodes_for_server_selects_group_or_falls_back() {
		let groups: Vec<EpGroup> = serde_json::from_str(SRC_DATA_JSON).unwrap();
		let eps = episodes_for_server(&groups, Some("Youtube"));
		assert_eq!(eps.len(), 3);
		assert_eq!(eps[0].key, "tap-1");
		assert_eq!(eps[0].episode_number, "1");
		// Unknown server → first group (Youtube).
		assert_eq!(episodes_for_server(&groups, Some("Nope")).len(), 3);
		assert_eq!(episodes_for_server(&groups, None).len(), 3);
		// Case-insensitive lookup.
		assert_eq!(episodes_for_server(&groups, Some("vietsub")).len(), 1);
		assert_eq!(episodes_for_server(&groups, Some("vietsub"))[0].key, "full");
	}

	#[komorei_test]
	fn lite_from_search_extracts_slug_from_url() {
		let item = SearchItem {
			name: "Điều Còn Thiếu".into(),
			origin_name: "The Missing Piece".into(),
			year: Some(2024),
			quality: Some("FHD".into()),
			episode_current: None,
			vote: Some(8.7),
			poster: Some("/uploads/poster.png".into()),
			url: Some("/phim/dieu-con-thieu".into()),
		};
		let anime = lite_from_search(&item, "https://kkphim.com");
		assert_eq!(anime.key, "dieu-con-thieu");
		assert_eq!(anime.title, "Điều Còn Thiếu");
		assert_eq!(anime.original_title, "The Missing Piece");
		assert_eq!(anime.release_year.as_ref().map(|y| y.name.as_str()), Some("2024"));
		assert_eq!(anime.cover, "https://kkphim.com/uploads/poster.png");
		assert_eq!(anime.url.as_deref(), Some("https://kkphim.com/phim/dieu-con-thieu"));
	}

	#[komorei_test]
	fn build_full_sorts_servers_by_episode_count() {
		let info = DetailInfo {
			episode_groups: serde_json::from_str(SRC_DATA_JSON).unwrap(),
			..Default::default()
		};
		let full = build_full(&info, "https://kkphim.com", "33-nguoi-tho-mo");
		// Largest server first → Youtube becomes season 1.
		assert_eq!(full.seasons.len(), 2);
		assert_eq!(full.seasons[0].title, "Youtube");
		assert_eq!(full.seasons[0].anime_id, "33-nguoi-tho-mo|Youtube");
		assert_eq!(full.seasons[1].title, "Vietsub");
		assert_eq!(full.episode_count, 3);
	}

	// ── HTML parsing (runner's hot host) ──────────────────────────────────

	#[komorei_test]
	fn parses_list_rows_into_lite_anime() {
		let doc = Html::parse(ROW_HTML).unwrap();
		let rows = parse_list_rows(&doc, "https://kkphim.com");
		assert_eq!(rows.len(), 2);

		let first = &rows[0];
		assert_eq!(first.key, "33-nguoi-tho-mo");
		assert_eq!(first.title, "33 Người Thợ Mỏ");
		assert_eq!(first.original_title, "The 33");
		assert_eq!(first.cover, "https://phimimg.com/upload/vod/20231113-1/bcd21adf3bfd735e07f5a2e2b9513743.jpg");
		assert_eq!(first.rating, Some(6.4));
		assert_eq!(first.release_year.as_ref().map(|y| y.name.as_str()), Some("2015"));
		assert_eq!(first.current_episode.as_deref(), Some("Full"));
		assert_eq!(first.status, AnimeStatus::Completed);
		assert_eq!(first.quality_tag.as_deref(), Some("Phim Lẻ"));
		assert_eq!(
			first.url.as_deref(),
			Some("https://kkphim.com/phim/33-nguoi-tho-mo")
		);

		let second = &rows[1];
		assert_eq!(second.key, "dieu-con-thieu");
		assert_eq!(second.title, "Điều Còn Thiếu");
		assert_eq!(second.current_episode.as_deref(), Some("Tập 6"));
		assert_eq!(second.status, AnimeStatus::Ongoing);
		assert_eq!(second.release_year.as_ref().map(|y| y.name.as_str()), Some("2024"));
	}

	#[komorei_test]
	fn parses_detail_metadata_and_srcdata() {
		let html = DETAIL_HTML.replace("###SRC_DATA_JSON###", SRC_DATA_JSON);
		let detail = parse_detail(&Html::parse(&html).unwrap());
		assert_eq!(detail.title, "33 Người Thợ Mỏ");
		assert_eq!(detail.origin, "The 33");
		assert_eq!(detail.year, Some(2015));
		assert_eq!(detail.lang.as_deref(), Some("Vietsub"));
		assert_eq!(detail.quality.as_deref(), Some("HD"));
		assert_eq!(detail.status.as_deref(), Some("completed"));
		assert_eq!(detail.current_episode.as_deref(), Some("Full"));
		assert_eq!(detail.rating, Some(6.4));
		assert_eq!(detail.rating_count, Some(1014));
		assert_eq!(detail.genres, vec!["Chính Kịch".to_string(), "Lịch Sử".to_string()]);
		assert_eq!(detail.countries, vec!["Âu Mỹ".to_string()]);
		let desc = detail.description.as_deref().unwrap_or("");
		assert!(desc.contains("33 người thợ mỏ"));
		assert_eq!(detail.episode_groups.len(), 2);
		assert_eq!(detail.episode_groups[0].server_name, "Youtube");
	}

	#[komorei_test]
	fn detects_next_page_from_pagination() {
		let doc = Html::parse(
			r#"<div>
				<ul class="pagination mb-0 d-none d-md-flex">
					<li class="page-item disabled"><span class="page-link">‹</span></li>
					<li class="page-item active"><span class="page-link">1</span></li>
					<li class="page-item"><a class="page-link" href="/danh-sach/phim-bo?page=2">2</a></li>
					<li class="page-item"><a class="page-link" href="/danh-sach/phim-bo?page=3">3</a></li>
				</ul>
			</div>"#,
		)
		.unwrap();
		assert!(has_next_page(&doc, 1));
		assert!(has_next_page(&doc, 2));
		assert!(!has_next_page(&doc, 3));
	}

	#[komorei_test]
	fn home_episode_stub_mirrors_grow_badge_and_date() {
		let doc = Html::parse(ROW_HTML).unwrap();
		let rows = doc.select("table.data-table tbody tr").unwrap();
		let row = rows.get(0).unwrap();
		let anime = parse_list_row(&row, "https://kkphim.com").unwrap();
		let ep = row_home_episode(&anime, &row);
		assert_eq!(ep.key, "33-nguoi-tho-mo__latest");
		assert_eq!(ep.episode_number, "1"); // "Full" has no digits
		assert_eq!(ep.date_uploaded, Some(1_790_027_735_000));
		assert_eq!(ep.thumbnail.as_deref(), Some(anime.cover.as_str()));
	}
}