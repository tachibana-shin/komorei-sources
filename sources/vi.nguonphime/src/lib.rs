//! # Nguồn Phim site source (`vi.nguonphime`)
//!
//! Scrapes the **nguonphime.site** movie website — the HTML front of the
//! "Nguồn 360" network (same box family as `ngontv.com`, `nguonphim.net`,
//! `phimtong.top`, …; the `api.nguonphim.net` REST backend is the separate
//! [vi.nguonphim](nguonphim) source). Unlike the API source this one runs on
//! the **HTML host** — every listing/detail page is server-rendered markup, so
//! all parsing here is jsoup selectors (a `vi.kkphim`-style HTML scrape).
//!
//! ## Anti-bot: the "NP Checker" bounce
//!
//! The FIRST cookie-less request of a session is 302-redirected to
//! `/site/site/embed/?url=<target>` (the checker interstitial), whose response
//! sets `PHPSESSID` + `us_session_id` cookies (plain HTTP `Set-Cookie`, no
//! JS) and then JS-forwards back. The app's OkHttp client follows the 302, so
//! a source request lands on the checker page body instead of the real page.
//! [fetch_html] detects the checker body (`<title>NP Checker</title>` /
//! the "Chào mừng…" greeting) and simply repeats the request once — the
//! cookies are already stored by the time of the retry. Everything else needs
//! no special handling (verified with plain curl: the grab embed pages answer
//! without cookies too).
//!
//! ## Endpoints & page shapes (verified against the live site)
//!
//! - **Listings** — `/` (Phim Hot), `/tuy-chon/phim-moi.html?ft=ne&ne=1`,
//!   `/tuy-chon/phim-bo.html?ft=ty&ty=2`, `/tuy-chon/phim-le.html?ft=ty&ty=1`,
//!   `/phim-{genre}-c{id}.html` (genre), `/tuy-chon/{name}.html?ft=co&co={code}`
//!   (country), `/tuy-chon/{year}.html?ft=ye&ye={year}`. Every list is a grid
//!   of `.item-file-index` cards (`a[href*="-f"]` + `img.hover-img` +
//!   `p.episode .current-episode`/`.total-episode` + a `.description` with
//!   country/year/views links). Pagination: `ul#yw0.Pager li.page a` with
//!   `?page=N`.
//! - **Search** — the header form posts `POST /tim-kiem-a.html`
//!   `{q, t:"film"}` (XHR, form-encoded) → JSON `{code:200, html}` where the
//!   html is the live-search dropdown: `li.result-item a[href*="-f"]` items.
//! - **Detail** — `/{slug}-f{id}.html` → `h1.title-2` (VN) + `p.subname`
//!   (original), cover `div.img-movie img`, plot `div.detail-film-desc`, and
//!   the `.infor-movie` paragraph rows (`Điểm`, `Đạo diễn`, `Quốc gia`,
//!   `Thể loại`, `Năm sản xuất`, `Đang phát: 24 / 47 Tập`). The detail page
//!   only lists the LATEST episodes ("Tập mới"); the FULL ordered episode list
//!   lives on the watch page `/xem-phim/{slug}-f{id}.html` as
//!   `a[href*="/xem-phim/"][href*="-e"]` (all 1193 One Piece anchors are in
//!   the page — the site just hides them with CSS/JS).
//! - **Streams** — two playback servers, both resolved headlessly:
//!   1. `POST /xem-phim/{…}.html` (XHR, body
//!      `fid={id}&time={unix}&key=…&loadTime=0&indexL=0&indexSL=0` — neither
//!      `time` nor `key` is validated server-side) → JSON `{code:200, html}`
//!      with a **signed** grab iframe `https://grab.nguonphime.site/embed/…`.
//!   2. GET the iframe → the player page hides the playlist as base64 inside
//!      `var v<hex> = "…"` (JWPlayer's decrypt is literally
//!      `JSON.parse(atob(…))`) → decode to `[{file, label, type, default,
//!      token, streamUrl}]`. **PAI** (`indexL=0`) yields a direct HLS file on
//!      `a.kvp726.com` with no required headers. **NGC** (`indexL=1`) is a
//!      second XHR to the page's own `var url = '…fromEmbed=1&api=…'` path
//!      → returns an iframe to `embed{N}.streamc.xyz/embed.php?hash=…` — the
//!      shared streamc grant engine (bootstrap → issue) also used by
//!      [vi.nguonc](nguonc)/[vi.nguonphim](nguonphim); its media needs the
//!      embed origin as `Referer`.
//!
//! ## Data model mapping to Komorei
//!
//! - `Anime.key` = the full detail path `"{slug}-f{id}"` (stable, deep-link
//!   friendly — site URLs are exactly `{base}/{key}.html`).
//! - **Season = one playback server** (like vi.ophim): PAI + NGC, hardcoded
//!   (every film on the network exposes the same two servers; the server list
//!   is only rendered inside the per-stream grab page).
//! - `Episode.key` = `"{number}-e{eid}"`, `Episode.url` = the watch page URL,
//!   and `get_stream` re-uses that URL for the watch XHR.
//!
//! [nguonphim]: ../nguonphim-source/index.html
//! [nguonc]: ../nguonc-source/index.html

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
	Source, StreamData, StreamInfo, StreamType, TextSetting,
	helpers::uri::encode_uri_component,
	imports::defaults::{DefaultValue, defaults_get, defaults_set},
	imports::html::{Document, Element, Html},
	imports::net::{Request, Response},
	imports::std::current_date,
	prelude::*,
	serde::Deserialize,
};

const SOURCE_ID: &str = "vi.nguonphime";
const DEFAULT_BASE: &str = "https://nguonphime.site";
const SETTING_BASE_URL: &str = "base_url";
const SETTING_LAST_NOTIFICATION: &str = "last_notification";

/// The stream grant always requests the plain unencrypted HLS format so the
/// app can play the result directly (no JS AES-GCM unwrapping in the player).
const PLAYLIST_FORMAT: &str = "hls";

/// Playback servers rendered by the grab player page (`li.serverItem
/// data-index`). Constant across the network; the season model maps one
/// Komorei season per server.
const SERVERS: &[&str] = &["PAI", "NGC"];

// ────────────────────────────────────────────────────────────────────────────
// Catalogs (name → URL slug → category id / filter code). All values were
// taken from the site's own nav menu and breadcrumb links.
// ────────────────────────────────────────────────────────────────────────────

/// Genre catalog: displayed name, URL slug, category id (`/phim-{slug}-c{id}.html`).
const GENRES: &[(&str, &str, &str)] = &[
	("Hành Động", "hanh-dong", "3"),
	("Võ Thuật", "vo-thuat", "4"),
	("Tâm Lý - Tình Cảm", "tam-ly-tinh-cam", "5"),
	("Hài Hước", "hai-huoc", "6"),
	("Hoạt Hình", "hoat-hinh", "7"),
	("Phiêu Lưu", "phieu-luu", "8"),
	("Kinh Dị", "kinh-di", "9"),
	("Hình Sự", "hinh-su", "10"),
	("Chiến Tranh", "chien-tranh", "11"),
	("Thần Thoại", "than-thoai", "12"),
	("Viễn Tưởng", "vien-tuong", "13"),
	("Cổ Trang", "co-trang", "14"),
	("Khoa Học Tài Liệu", "khoa-hoc-tai-lieu", "15"),
	("Âm Nhạc", "am-nhac", "16"),
	("Phim 18+", "18", "17"),
	("Chiếu Rạp", "chieu-rap", "18"),
	("Xã Hội Đen", "xa-hoi-den", "19"),
	("Việt Xưa", "viet-xua", "20"),
];

/// Country catalog: displayed name, URL slug, ISO code
/// (`/tuy-chon/{slug}.html?ft=co&co={code}`).
const COUNTRIES: &[(&str, &str, &str)] = &[
	("Trung Quốc", "trung-quoc", "CN"),
	("Hàn Quốc", "han-quoc", "KR"),
	("Nhật Bản", "nhat-ban", "JP"),
	("Thái Lan", "thai-lan", "TH"),
	("Việt Nam", "viet-nam", "VN"),
	("Hồng Kông", "hong-kong", "HK"),
	("Đài Loan", "dai-loan", "TW"),
	("Mỹ", "my", "US"),
	("Pháp", "phap", "FR"),
	("Anh", "anh", "GB"),
	("Ấn Độ", "an-do", "IN"),
	("Indonesia", "indonesia", "ID"),
	("Malaysia", "malaysia", "MY"),
	("Mexico", "mexico", "MX"),
	("Brazil", "brazil", "BR"),
	("Tây Ban Nha", "tay-ban-nha", "ES"),
	("Úc", "uc", "AU"),
];

/// "Loại phim" select + home rails: name, slug, filter key, filter value
/// (`/tuy-chon/{slug}.html?ft={key}&{key}={value}`).
const TYPE_FILTERS: &[(&str, &str, &str, &str)] = &[
	("Phim Hot", "phim-hot", "ho", "1"),
	("Phim Mới", "phim-moi", "ne", "1"),
	("Phim Lẻ", "phim-le", "ty", "1"),
	("Phim Bộ", "phim-bo", "ty", "2"),
];

const YEAR_FIRST: i32 = 1998;
const YEAR_LAST: i32 = 2026;

fn year_options() -> Vec<String> {
	(YEAR_FIRST..=YEAR_LAST).map(|y| format!("{y}")).collect()
}

/// Dynamic listings — each id IS the site's relative list path.
const LISTINGS: &[(&str, &str)] = &[
	("tuy-chon/phim-moi.html?ft=ne&ne=1", "Mới Cập Nhật"),
	("tuy-chon/phim-hot.html?ft=ho&ho=1", "Phim Hot"),
	("tuy-chon/phim-bo.html?ft=ty&ty=2", "Phim Bộ"),
	("tuy-chon/phim-le.html?ft=ty&ty=1", "Phim Lẻ"),
	("phim-chieu-rap-c18.html", "Chiếu Rạp"),
	("phim-hoat-hinh-c7.html", "Hoạt Hình"),
	("phim-hanh-dong-c3.html", "Hành Động"),
	("phim-co-trang-c14.html", "Cổ Trang"),
];

// ────────────────────────────────────────────────────────────────────────────
// JSON structs
// ────────────────────────────────────────────────────────────────────────────

/// The watch-page XHR reply: `{code:200, html:"<iframe id=playerEmbed …>"}`.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct WatchPostResp {
	code: i32,
	html: String,
}

/// One entry of the obfuscated playlist array hidden in the grab page. The
/// network encodes `[{file,label,type,default[,streamUrl,token]}]` as base64
/// and defers `JSON.parse(atob(…))` to the player JS — this source decodes it
/// natively. A `token` entry means the CDN wants the token header (and the JS
/// rewrites `/key` segment URLs onto `streamUrl`).
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct PlaylistEntry {
	file: String,
	label: Option<String>,
	#[serde(rename = "type")]
	kind: Option<String>,
	default: Option<bool>,
	#[serde(rename = "streamUrl")]
	stream_url: Option<String>,
	token: Option<String>,
}

/// The streamc bootstrap response — the `bootstrap` field is the grant token
/// handed to the `issue` POST.
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

/// First run of decimal digits ("Tập 24" → "24").
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

/// First run of decimal digits as an integer (""/"1970xyz" → None).
fn parse_first_int(input: Option<&str>) -> Option<i32> {
	let input = input?;
	let digits: String = input
		.chars()
		.skip_while(|c| !c.is_ascii_digit())
		.take_while(|c| c.is_ascii_digit())
		.collect();
	digits.parse().ok()
}

/// First decimal number (digits + `.`/`,` separators) after the first `:`.
fn parse_first_f32(input: &str) -> Option<f32> {
	let idx = input.find(':')? + 1;
	let v: String = input[idx..]
		.trim_start()
		.chars()
		.take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
		.collect();
	v.replace(',', ".").parse().ok()
}

/// The first two integers in a string ("24 / 47 Tập" → (Some(24), Some(47))).
fn parse_two_ints(input: &str) -> (Option<i32>, Option<i32>) {
	let nums: Vec<i32> = input
		.split(|c: char| !c.is_ascii_digit())
		.filter(|s| !s.is_empty())
		.filter_map(|s| s.parse().ok())
		.collect();
	(nums.first().copied(), nums.get(1).copied())
}

/// Parse `?page=N` (also `&page=N`) out of a pager href.
fn page_param(href: &str) -> Option<i32> {
	let idx = href.find("page=")?;
	let digits: String = href[idx + 5..]
		.chars()
		.take_while(|c| c.is_ascii_digit())
		.collect();
	digits.parse().ok()
}

/// The Last page number reachable via `ul#yw0.Pager`; has next if any page
/// link exceeds the current page.
fn has_next_page(doc: &Document, page: i32) -> bool {
	let mut max_page = page;
	if let Some(links) = doc.select("ul.Pager li.page a[href]") {
		for i in 0..links.size() {
			if let Some(a) = links.get(i)
				&& let Some(href) = a.attr("href")
				&& let Some(p) = page_param(&href)
			{
				max_page = max_page.max(p);
			}
		}
	}
	max_page > page
}

/// Film id from `Anime.key = "{slug}-f{id}"` (the trailing `-f\d+` part).
fn film_id(key: &str) -> Option<String> {
	let idx = key.rfind("-f")?;
	let digits: String = key[idx + 2..]
		.chars()
		.take_while(|c| c.is_ascii_digit())
		.collect();
	if digits.is_empty() {
		None
	} else {
		Some(digits)
	}
}

/// `"lan-huong-nhu-co-f83892-24-e1007951.html"` → `"lan-huong-nhu-co-f83892"`
/// (the deep-link / watch-path anime key).
fn film_key_prefix(segment: &str) -> Option<String> {
	let seg = segment.trim_end_matches(".html");
	let idx = seg.rfind("-f")?;
	let digits: String = seg[idx + 2..]
		.chars()
		.take_while(|c| c.is_ascii_digit())
		.collect();
	if digits.is_empty() {
		return None;
	}
	Some(seg[..idx + 2 + digits.len()].to_string())
}

/// Scheme+host origin; `Referer`/`Origin` for the streamc grant and media.
fn embed_origin(url: &str) -> Option<String> {
	let (scheme, rest) = if let Some(rest) = url.strip_prefix("https://") {
		("https://", rest)
	} else if let Some(rest) = url.strip_prefix("http://") {
		("http://", rest)
	} else {
		return None;
	};
	let host = rest.split(['/', '?', '#']).next().filter(|h| !h.is_empty())?;
	Some(format!("{scheme}{host}"))
}

/// Absolute cover URLs (the nps3 CDN) are already absolute; just drop empties.
fn absolutize(url: String) -> Option<String> {
	if url.is_empty() {
		None
	} else {
		Some(url)
	}
}

/// The NP Checker body marker — the interstitial page a cookie-less request
/// is bounced through (`<title>NP Checker</title>` + a JS forward + a
/// "Chào mừng…" greeting).
fn is_checker_page(body: &str) -> bool {
	body.contains("NP Checker") || body.contains("Chào mừng bạn đến với chúng tôi")
}

/// RFC 4648 standard-alphabet base64 decode (alloc). Returns `None` on any
/// character outside the alphabet (`=`-padding tolerated at the end).
fn b64_decode(input: &str) -> Option<Vec<u8>> {
	let bytes: Vec<u8> = input
		.as_bytes()
		.iter()
		.copied()
		.filter(|b| !b.is_ascii_whitespace())
		.collect();
	if bytes.is_empty() || bytes.len() % 4 != 0 {
		return None;
	}
	let val = |b: u8| -> Option<u32> {
		match b {
			b'A'..=b'Z' => Some(u32::from(b - b'A')),
			b'a'..=b'z' => Some(u32::from(b - b'a') + 26),
			b'0'..=b'9' => Some(u32::from(b - b'0') + 52),
			b'+' => Some(62),
			b'/' => Some(63),
			b'=' => Some(0), // padding handled by `emit` below
			_ => None,
		}
	};
	let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
	for chunk in bytes.chunks(4) {
		let pads = chunk.iter().filter(|&&b| b == b'=').count();
		let data_chars = 4 - pads;
		let mut acc: u32 = 0;
		for &b in chunk {
			if b == b'=' {
				break; // padding only ever closes a chunk
			}
			acc = (acc << 6) | val(b)?;
		}
		let emit = data_chars.saturating_sub(1);
		let total_bits = data_chars * 6;
		for k in 0..emit {
			let shift = total_bits - 8 - k * 8;
			out.push(((acc >> shift) & 0xFF) as u8);
		}
	}
	Some(out)
}

/// Scan raw HTML for `"…"`-quoted base64 tokens and decode the first one that
/// parses as a playlist array (a `[{file,…}]` JSON). This mirrors the player's
/// `JSON.parse(atob(v<hex>))`.
fn extract_playlist(html: &str) -> Option<Vec<PlaylistEntry>> {
	let bytes = html.as_bytes();
	let mut i = 0;
	while i < bytes.len() {
		if bytes[i] != b'"' {
			i += 1;
			continue;
		}
		let mut token = String::new();
		let mut j = i + 1;
		while j < bytes.len() && bytes[j] != b'"' {
			token.push(char::from(bytes[j]));
			j += 1;
		}
		if token.len() >= 60
			&& let Some(raw) = b64_decode(&token)
			&& raw.first() == Some(&b'[')
			&& let Ok(entries) = serde_json::from_slice::<Vec<PlaylistEntry>>(&raw)
			&& entries.iter().any(|e| !e.file.is_empty())
		{
			return Some(entries);
		}
		i = j;
		i += 1;
	}
	None
}

/// Extract the server-switch POST path from the grab page:
/// `var url = '/xem-phim/…?key=…&tim=…&fromEmbed=1&api=nguonphime.site';`.
/// The player also declares `var url = ''` early in the page, so scan for the
/// declaration that actually carries `fromEmbed=1`.
fn from_embed_url(html: &str) -> Option<String> {
	let marker = "var url = '";
	let mut search_from = 0;
	while let Some(rel) = html[search_from..].find(marker) {
		let idx = search_from + rel + marker.len();
		let rest = &html[idx..];
		let end = rest.find('\'').unwrap_or(rest.len());
		let url = &rest[..end];
		if !url.is_empty() && url.contains("fromEmbed=1") {
			return Some(url.to_string());
		}
		search_from = idx + 1;
	}
	None
}

/// `key`/`tim`/any single query param out of a path (for the fromEmbed POST).
fn query_param(path: &str, name: &str) -> Option<String> {
	let idx = path.find(name)?;
	let after = &path[idx + name.len()..];
	let after = after.strip_prefix('=')?;
	let value: String = after
		.chars()
		.take_while(|c| *c != '&')
		.collect();
	if value.is_empty() {
		None
	} else {
		Some(value)
	}
}

// ────────────────────────────────────────────────────────────────────────────
// HTML parsing
// ────────────────────────────────────────────────────────────────────────────

/// Parse a listing/grid card (`.item-file-index`) into a Lite anime.
fn from_card(card: &Element, base: &str) -> Option<Anime> {
	let link = card.select_first("a[href*='-f']")?;
	let href = link.attr("href")?;
	let path = href.trim_start_matches('/');
	if !path.contains("-f") || !path.ends_with(".html") {
		return None;
	}
	let key = path.trim_end_matches(".html").to_string();

	let title = link.attr("title").unwrap_or_default();
	if title.trim().is_empty() {
		return None;
	}
	let cover = card
		.select_first("img.hover-img")
		.and_then(|i| i.attr("src"))
		.and_then(absolutize);

	let current = card
		.select_first("p.episode .current-episode")
		.and_then(|e| e.text())
		.unwrap_or_default();
	let total = card
		.select_first("p.episode .total-episode")
		.and_then(|e| e.text())
		.unwrap_or_default();

	let year = card
		.select_first(".description a[href*='ft=ye']")
		.and_then(|a| a.attr("title"));
	let country = card
		.select_first(".description a[href*='ft=co']")
		.and_then(|a| a.attr("title"));
	let views = card
		.select_first(".description .fa-eye")
		.and_then(|i| i.parent())
		.and_then(|p| p.text())
		.and_then(|t| parse_first_int(Some(&t)))
		.unwrap_or(0);

	let current_num = parse_first_int(Some(&current));
	let total_num = parse_first_int(Some(&total));
	let completed = matches!((current_num, total_num), (Some(c), Some(t)) if t > 0 && c >= t);

	Some(Anime {
		key,
		source_id: SOURCE_ID.into(),
		title: title.trim().to_string(),
		original_title: String::new(),
		cover: cover.clone().unwrap_or_default(),
		banner: cover,
		description: None,
		episode_count: total_num.unwrap_or(0),
		current_episode: current_num.map(|n| format!("Tập {n}")),
		rating: None,
		rating_count: None,
		status: None.or(if completed { Some(AnimeStatus::Completed) } else { None })
			.unwrap_or(AnimeStatus::Unknown),
		release_year: parse_first_int(year.as_deref()).map(|y| CategoryLink {
			name: format!("{y}"),
			filters: Vec::new(),
		}),
		genres: Vec::new(),
		authors: Vec::new(),
		studio: None,
		season_of: None,
		countries: country
			.map(|c| vec![CategoryLink {
				name: c,
				filters: Vec::new(),
			}])
			.unwrap_or_default(),
		is_featured: false,
		views,
		next_episode_air_info: None,
		quality_tag: None,
		seasons: Vec::new(),
		episodes: None,
		url: Some(link_base(base, &path)),
	})
}

/// `base` + the `/…` path the card linked to.
fn link_base(base: &str, path: &str) -> String {
	format!("{base}/{path}")
}

/// All link texts inside an element (genre/country/director rows).
fn link_texts(e: &Element) -> Vec<String> {
	let mut out = Vec::new();
	if let Some(links) = e.select("a") {
		for i in 0..links.size() {
			if let Some(a) = links.get(i)
				&& let Some(t) = a.text()
				&& !t.trim().is_empty()
			{
				out.push(t.trim().to_string());
			}
		}
	}
	out
}

fn first_text(doc: &Document, selector: &str) -> Option<String> {
	doc.select_first(selector).and_then(|e| e.text())
}

/// Every parsed metadata slice the source needs from a detail page.
#[derive(Default)]
struct DetailInfo {
	title: String,
	subname: String,
	cover: Option<String>,
	description: String,
	score: Option<f32>,
	directors: Vec<String>,
	genres: Vec<String>,
	countries: Vec<String>,
	year: Option<String>,
	current_episode: Option<i32>,
	total_episodes: Option<i32>,
}

fn parse_detail(doc: &Document) -> DetailInfo {
	let mut info = DetailInfo {
		title: first_text(doc, "h1.title-2").unwrap_or_default(),
		subname: first_text(doc, "p.subname").unwrap_or_default(),
		cover: doc
			.select_first(".img-movie img")
			.and_then(|i| i.attr("src"))
			.and_then(absolutize),
		description: first_text(doc, ".detail-film-desc").unwrap_or_default(),
		..Default::default()
	};
	if let Some(ps) = doc.select(".infor-movie p") {
		for i in 0..ps.size() {
			let Some(p) = ps.get(i) else { continue };
			let text = p.text().unwrap_or_default();
			let text = text.trim();
			if text.starts_with("Điểm") {
				info.score = parse_first_f32(text);
			} else if text.starts_with("Đạo diễn") || text.starts_with("Diễn viên") {
				info.directors = link_texts(&p);
			} else if text.starts_with("Thể loại") {
				info.genres = link_texts(&p);
			} else if text.starts_with("Quốc gia") {
				info.countries = link_texts(&p);
			} else if text.starts_with("Năm") {
				info.year = link_texts(&p).first().cloned();
			} else if text.starts_with("Đang phát") || text.starts_with("Dừng phát") {
				let (cur, total) = parse_two_ints(text);
				info.current_episode = cur;
				info.total_episodes = total;
			}
		}
	}
	info
}

/// Full ordered episode list from the watch page
/// (`a[href*="/xem-phim/"][href*="-e"]`).
fn parse_episodes(doc: &Document, base: &str, _film_key: &str) -> Vec<Episode> {
	let mut out: Vec<Episode> = Vec::new();
	let Some(links) = doc.select("a[href*='/xem-phim/'][href*='-e']") else {
		return out;
	};
	for i in 0..links.size() {
		let Some(a) = links.get(i) else { continue };
		let Some(href) = a.attr("href") else { continue };
		let number = a.text().unwrap_or_default().trim().to_string();
		if number.is_empty() {
			continue;
		}
		// `/xem-phim/{slug}-f{id}-{ep}-e{eid}.html` — keep the original href
		// (with `.html`) as the playable url.
		let path = href.trim_start_matches('/');
		let Some(eidx) = path.rfind("-e") else { continue };
		let eid: String = path[eidx + 2..]
			.chars()
			.take_while(|c| c.is_ascii_digit())
			.collect();
		if eid.is_empty() {
			continue;
		}
		let ep_num = parse_episode_number(&number);
		let key = format!("{ep_num}-e{eid}");
		if out.iter().any(|e| e.key == key) {
			continue;
		}
		out.push(Episode {
			key,
			episode_number: ep_num,
			title: None,
			date_uploaded: None,
			thumbnail: None,
			quality: None,
			duration_seconds: None,
			url: Some(link_base(base, path)),
			language: Some(String::from("vi")),
			locked: false,
		});
	}
	out
}

/// Parse a grid/list page: cards + pager.
fn parse_list_page(doc: &Document, base: &str, page: i32) -> (Vec<Anime>, bool) {
	let mut out: Vec<Anime> = Vec::new();
	if let Some(cards) = doc.select(".item-file-index") {
		for i in 0..cards.size() {
			if let Some(card) = cards.get(i)
				&& let Some(anime) = from_card(&card, base)
				&& !out.iter().any(|e| e.key == anime.key)
			{
				out.push(anime);
			}
		}
	}
	(out, has_next_page(doc, page))
}

/// Parse the live-search dropdown (`li.result-item a[href*="-f"]`).
fn parse_search_items(doc: &Document, base: &str) -> Vec<Anime> {
	let mut out: Vec<Anime> = Vec::new();
	let Some(items) = doc.select("li.result-item a[href]") else {
		return out;
	};
	for i in 0..items.size() {
		let Some(a) = items.get(i) else { continue };
		let Some(href) = a.attr("href") else { continue };
		let path = href.trim_start_matches('/');
		if !path.contains("-f") || !path.ends_with(".html") {
			continue;
		}
		let title = a.attr("title").unwrap_or_default().trim().to_string();
		if title.is_empty() {
			continue;
		}
		let original = a
			.select_first(".result-item-title-en")
			.and_then(|e| e.text())
			.map(|t| t.trim().to_string())
			.unwrap_or_default();
		let cover = a
			.select_first("img")
			.and_then(|i| i.attr("src"))
			.and_then(absolutize);
		let key = path.trim_end_matches(".html").to_string();
		if out.iter().any(|e| e.key == key) {
			continue;
		}
		out.push(Anime {
			cover: cover.clone().unwrap_or_default(),
			banner: cover,
			original_title: original,
			url: Some(link_base(base, path)),
			..anime_lite(key, title)
		});
	}
	out
}

/// A Lite card scaffold with the source id wired; field defaults are filled by
/// the callers (cover/url/etc.).
fn anime_lite(key: String, title: String) -> Anime {
	Anime {
		key,
		source_id: SOURCE_ID.into(),
		title,
		original_title: String::new(),
		cover: String::new(),
		banner: None,
		description: None,
		episode_count: 0,
		current_episode: None,
		rating: None,
		rating_count: None,
		status: AnimeStatus::Unknown,
		release_year: None,
		genres: Vec::new(),
		authors: Vec::new(),
		studio: None,
		season_of: None,
		countries: Vec::new(),
		is_featured: false,
		views: 0,
		next_episode_air_info: None,
		quality_tag: None,
		seasons: Vec::new(),
		episodes: None,
		url: None,
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Fetch + build
// ────────────────────────────────────────────────────────────────────────────

/// Parse an HTML body, transparently surviving the NP Checker bounce (see the
/// module docs): detect the checker body and re-request once — the session
/// cookies were stored by the app's cookie jar on the first hop.
fn fetch_html(url: &str) -> Result<Document> {
	let parse = |body: String| -> Result<Document> {
		Html::parse(body).map_err(|_| error!("Máy chủ trả về HTML không hợp lệ."))
	};

	let body = Request::get(url)?.string()?;
	if !is_checker_page(&body) {
		return parse(body);
	}
	// Bounced → session cookies are now in the jar; retry the real page once.
	let body = Request::get(url)?.string()?;
	if is_checker_page(&body) {
		bail!("Máy chủ nguồn từ chối (NP Checker): {url}");
	}
	parse(body)
}

/// Raw body (used for grab pages that only carry JS-encoded playlists).
fn fetch_body(url: &str) -> Result<String> {
	Request::get(url)?.string()
}

/// Form-encoded XHR POST (the watch-page player request + the fromEmbed
/// server switch). A cookie-less first POST is bounced through the NP Checker
/// too — its HTML body cannot decode as JSON. In that case the session is
/// warmed with a base GET (which bounces once, storing the cookies) and the
/// POST is retried once, mirroring how a real browser session starts.
fn watch_post(url: &str, body: &str, base: &str, referer: &str) -> Result<WatchPostResp> {
	let send = || -> Result<Response> {
		let request = Request::post(url)
			.map_err(|_| error!("Liên kết nguồn phát không hợp lệ."))?
			.header("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8")
			.header("X-Requested-With", "XMLHttpRequest")
			.header("Origin", base)
			.header("Referer", referer)
			.body(body);
		let resp = request
			.send()
			.map_err(|_| error!("Không kết nối được máy chủ nguồn."))?;
		let status = resp.status_code();
		if status < 200 || status >= 300 {
			bail!("Máy chủ nguồn phát lỗi {status}");
		}
		Ok(resp)
	};

	let text = send()?
		.get_string()
		.map_err(|_| error!("Máy chủ nguồn trả dữ liệu không hợp lệ."))?;
	if let Ok(parsed) = serde_json::from_str::<WatchPostResp>(&text) {
		return Ok(parsed);
	}
	if is_checker_page(&text) {
		// Bounce the NP Checker open (its first GET stores the session
		// cookies) and retry the POST once with the fresh session.
		let _ = fetch_html(&format!("{base}/"));
		let ok = send()?;
		return ok
			.get_json_owned()
			.map_err(|_| error!("Máy chủ nguồn trả dữ liệu không hợp lệ."));
	}
	bail!("Máy chủ nguồn trả dữ liệu không hợp lệ.")
}

/// One watch-page player POST → the signed grab iframe URL. `time`/`key` are
/// not validated by the server, so the unix clock and the server name are
/// sent; `indexL` selects the playback server for the first resolution.
fn watch_iframe_url(base: &str, watch_url: &str, fid: &str, index_l: u8) -> Result<String> {
	let body = format!(
		"fid={fid}&time={}&key=PAI&loadTime=0&indexL={index_l}&indexSL=0",
		current_date()
	);
	let parsed = watch_post(watch_url, &body, base, watch_url)?;
	if parsed.code != 200 || parsed.html.is_empty() {
		bail!("Máy chủ nguồn chưa sẵn sàng tập này (mã {})", parsed.code);
	}
	let doc = Html::parse(parsed.html).map_err(|_| error!("Trang phát trả về HTML không hợp lệ."))?;
	let src = doc
		.select_first("iframe#playerEmbed")
		.and_then(|f| f.attr("src"))
		.filter(|s| !s.is_empty())
		.map(|s| if s.starts_with("//") { format!("https:{s}") } else { s })
		.ok_or_else(|| error!("Không tìm thấy liên kết phát (iframe)."))?;
	Ok(src)
}

/// Resolve the **PAI** playback server: grab page → base64 playlist → the
/// direct HLS url. A `token`-bearing entry would need the token header + the
/// `/key` segment URL rewrite onto `streamUrl`; none of the verified films use
/// it, but the handling is cheap and mirrors the player's `onXhrOpen`.
fn resolve_pai(iframe_url: &str) -> Result<(String, komorei::HashMap<String, String>)> {
	let grab = fetch_body(iframe_url)?;
	let entry = extract_playlist(&grab)
		.and_then(|list| list.into_iter().find(|e| !e.file.is_empty()))
		.ok_or_else(|| error!("Không tìm thấy nguồn phát trên trang embed."))?;

	let mut headers = komorei::HashMap::new();
	let mut url = entry.file;
	if let Some(token) = entry.token.filter(|t| !t.is_empty()) {
		headers.insert(String::from("token"), token);
		if let Some(stream_url) = entry.stream_url.filter(|s| !s.is_empty())
			&& url.contains("/key")
		{
			// The player swaps the `/key` segment host for the token CDN.
			if let Some(origin) = embed_origin(&url) {
				url = url.replacen(&origin, &stream_url, 1);
			}
		}
	}
	Ok((url, headers))
}

/// Match the network's own embedded behavior … the NGC server is
/// `embed{N}.streamc.xyz` behind the *site's* fromEmbed POST.
fn resolve_ngc(base: &str, watch_url: &str, fid: &str) -> Result<(String, komorei::HashMap<String, String>)> {
	let iframe = watch_iframe_url(base, watch_url, fid, 0)?;
	let grab = fetch_body(&iframe)?;
	let embed_path = from_embed_url(&grab)
		.ok_or_else(|| error!("Trang embed không kích hoạt được máy chủ phụ."))?;
	let tim = query_param(&embed_path, "tim").unwrap_or_else(|| current_date().to_string());
	let body = format!("fid={fid}&time={tim}&key=NGC&loadTime=0&indexL=1&indexSL=0");
	let parsed = watch_post(&format!("{base}{embed_path}"), &body, base, watch_url)?;
	let doc = Html::parse(parsed.html).map_err(|_| error!("Trang phát trả về HTML không hợp lệ."))?;
	let embed = doc
		.select_first("iframe#playerEmbed")
		.and_then(|f| f.attr("src"))
		.filter(|s| !s.is_empty())
		.ok_or_else(|| error!("Máy chủ phụ không trả liên kết phát."))?;

	let origin = embed_origin(&embed).ok_or_else(|| error!("Liên kết phát không hợp lệ: {embed}"))?;
	let playlist = resolve_streamc_playlist(&embed, watch_url, &origin)?;
	let mut headers = komorei::HashMap::new();
	headers.insert(String::from("Referer"), format!("{origin}/"));
	Ok((playlist, headers))
}

/// Walk the streamc embed's two-step grant (bootstrap → issue) and return the
/// signed HLS playlist URL. `referrer` is the film page the embed would be
/// loaded from; `origin` becomes the `Origin`/`Referer` on the grant POSTs and
/// the media `Referer` for the segment CDN.
fn resolve_streamc_playlist(embed_url: &str, referrer: &str, origin: &str) -> Result<String> {
	let post_json = |body: &str| -> Result<serde_json::Value> {
		let request = Request::post(embed_url)
			.map_err(|_| error!("Địa chỉ nguồn phát không hợp lệ."))?
			.header("Content-Type", "application/json")
			.header("Origin", origin)
			.header("Referer", origin)
			.body(body);
		let resp = request.send().map_err(|_| error!("Không kết nối được máy chủ nguồn phát."))?;
		let status = resp.status_code();
		if status < 200 || status >= 300 {
			bail!("Máy chủ nguồn phát lỗi {status}");
		}
		resp.get_json_owned::<serde_json::Value>()
			.map_err(|_| error!("Máy chủ nguồn phát trả dữ liệu không hợp lệ."))
	};

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
	let bootstrap: BootstrapResp = serde_json::from_value(post_json(&bootstrap_body.to_string())?)
		.map_err(|_| error!("Máy chủ nguồn phát trả dữ liệu không hợp lệ."))?;
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
	let issue: IssueResp = serde_json::from_value(post_json(&issue_body.to_string())?)
		.map_err(|_| error!("Máy chủ nguồn phát trả dữ liệu không hợp lệ."))?;
	let playlist = issue
		.playlist
		.ok_or_else(|| error!("Không lấy được nguồn phát video."))?;

	if let Some(fmt) = issue.playlist_format.as_deref()
		&& fmt != PLAYLIST_FORMAT
	{
		bail!("Nguồn phát dùng định dạng mã hoá ({fmt}), app chưa hỗ trợ.");
	}
	Ok(playlist)
}

/// Build the site listing url from search filters (first selected wins:
/// type → genre → country → year, then "Mới Cập Nhật").
fn build_list_url(base: &str, page: i32, filters: &[FilterValue]) -> String {
	if let Some(t) = select_value(filters, "type")
		&& let Some((_, slug, key, val)) = TYPE_FILTERS
			.iter()
			.find(|(name, ..)| name.eq_ignore_ascii_case(t))
	{
		return format!("{base}/tuy-chon/{slug}.html?ft={key}&{key}={val}&page={page}");
	}
	if let Some(g) = multi_included(filters, "genre")
		&& let Some((_, slug, cid)) = GENRES
			.iter()
			.find(|(name, ..)| name.eq_ignore_ascii_case(g))
	{
		return format!("{base}/phim-{slug}-c{cid}.html?page={page}");
	}
	if let Some(c) = multi_included(filters, "country")
		&& let Some((_, slug, code)) = COUNTRIES
			.iter()
			.find(|(name, ..)| name.eq_ignore_ascii_case(c))
	{
		return format!("{base}/tuy-chon/{slug}.html?ft=co&co={code}&page={page}");
	}
	if let Some(y) = select_value(filters, "year")
		&& let Some(year) = parse_first_int(Some(y))
	{
		return format!("{base}/tuy-chon/{year}.html?ft=ye&ye={year}&page={page}");
	}
	format!("{base}/tuy-chon/phim-moi.html?ft=ne&ne=1&page={page}")
}

fn select_value<'a>(filters: &'a [FilterValue], id: &str) -> Option<&'a str> {
	filters.iter().find_map(|f| match f {
		FilterValue::Select { id: f_id, value } if f_id == id => Some(value.as_str()),
		_ => None,
	})
}

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

/// Full detail: metadata + seasons (PAI/NGC).
fn build_full(base: &str, key: &str, info: DetailInfo) -> Anime {
	let status = match (info.current_episode, info.total_episodes) {
		(Some(cur), Some(total)) if total > 0 && cur >= total => AnimeStatus::Completed,
		(Some(_), Some(_)) => AnimeStatus::Ongoing,
		_ => AnimeStatus::Unknown,
	};

	let seasons: Vec<AnimeSeason> = SERVERS
		.iter()
		.map(|name| AnimeSeason {
			anime_id: format!("{key}|{name}"),
			title: (*name).to_string(),
			id: format!("{key}|{name}"),
		})
		.collect();

	Anime {
		key: key.to_string(),
		source_id: SOURCE_ID.into(),
		title: info.title,
		original_title: info.subname,
		cover: info.cover.clone().unwrap_or_default(),
		banner: info.cover,
		description: Some(info.description),
		episode_count: info.total_episodes.unwrap_or(0),
		current_episode: info.current_episode.map(|n| format!("Tập {n}")),
		rating: info.score,
		rating_count: None,
		status,
		release_year: parse_first_int(info.year.as_deref()).map(|y| CategoryLink {
			name: format!("{y}"),
			filters: Vec::new(),
		}),
		genres: info
			.genres
			.into_iter()
			.map(|name| CategoryLink { name, filters: Vec::new() })
			.collect(),
		authors: info
			.directors
			.into_iter()
			.map(|name| CategoryLink { name, filters: Vec::new() })
			.collect(),
		studio: None,
		season_of: None,
		countries: info
			.countries
			.into_iter()
			.map(|name| CategoryLink { name, filters: Vec::new() })
			.collect(),
		is_featured: false,
		views: 0,
		next_episode_air_info: None,
		quality_tag: None,
		seasons,
		episodes: None,
		url: Some(format!("{base}/{key}.html")),
	}
}

/// Episode badge for the "Mới Cập Nhật" home rail.
fn home_episode(card: &Anime) -> Episode {
	Episode {
		key: format!("mc-{}-{}", card.key, card.current_episode.as_deref().unwrap_or("0")),
		episode_number: parse_episode_number(card.current_episode.as_deref().unwrap_or("1")),
		title: Some(card.current_episode.clone().unwrap_or_else(|| String::from("Tập mới"))),
		date_uploaded: None,
		thumbnail: None,
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

pub struct NguonphimeSource;

impl NguonphimeSource {
	fn base(&self) -> String {
		match defaults_get::<String>(SETTING_BASE_URL) {
			Some(s) if !s.trim().is_empty() => s.trim().to_string(),
			_ => String::from(DEFAULT_BASE),
		}
	}

	/// One listing page (cards + pager), with the checker bounce handled.
	fn page(&self, url: &str, page: i32) -> Result<(Vec<Anime>, bool)> {
		let doc = fetch_html(url)?;
		Ok(parse_list_page(&doc, &self.base(), page))
	}

	/// Fetch `{base}/{key}.html` and parse the metadata rows; `None` when the
	/// page is not a film detail (unknown key).
	fn detail(&self, base: &str, key: &str) -> Result<Option<DetailInfo>> {
		let doc = fetch_html(&format!("{base}/{key}.html"))?;
		let info = parse_detail(&doc);
		if info.title.is_empty() {
			Ok(None)
		} else {
			Ok(Some(info))
		}
	}
}

impl Source for NguonphimeSource {
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
		let q = query.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
		let (entries, has_next_page) = match q {
			Some(q) => {
				// Live-search XHR: POST /tim-kiem-a.html {q, t:"film"} → JSON html.
				let body = format!(
					"q={}&t=film",
					encode_uri_component(q.as_str())
				);
				let parsed = watch_post(
					&format!("{base}/tim-kiem-a.html"),
					&body,
					&base,
					&format!("{base}/"),
				)?;
				let doc = Html::parse(parsed.html)
					.map_err(|_| error!("Kết quả tìm kiếm không hợp lệ."))?;
				(parse_search_items(&doc, &base), false)
			}
			None => {
				let url = build_list_url(&base, page, &filters);
				self.page(&url, page)?
			}
		};
		Ok(AnimePageResult {
			entries,
			has_next_page,
		})
	}

	fn get_anime_update(
		&self,
		mut anime: Anime,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Anime> {
		let key = anime.key.clone();
		let base = self.base();
		if needs_details && let Some(info) = self.detail(&base, &key)? {
			anime.copy_from(build_full(&base, &key, info));
		}
		if needs_chapters {
			let doc = fetch_html(&format!("{base}/xem-phim/{key}.html"))?;
			anime.episodes = Some(parse_episodes(&doc, &base, &key));
		}
		Ok(anime)
	}

	fn get_stream_list(&self, anime: Anime, _episode: Episode) -> Result<Vec<StreamInfo>> {
		let quality = anime.quality_tag.clone().unwrap_or_default();
		Ok(SERVERS
			.iter()
			.map(|name| StreamInfo {
				key: (*name).to_string(),
				name: (*name).to_string(),
				quality: quality.clone(),
			})
			.collect())
	}

	fn get_stream(&self, anime: Anime, episode: Episode, stream: StreamInfo) -> Result<StreamData> {
		let base = self.base();
		let fid = film_id(&anime.key).ok_or_else(|| {
			error!("Không xác định được mã phim: {}", anime.key)
		})?;
		// The watch URL is stored on the episode during needs_chapters; fall
		// back to the canonical `{base}/xem-phim/{key}-{ep}-e{eid}.html` shape.
		let watch_url = episode.url.unwrap_or_else(|| {
			format!("{base}/xem-phim/{}-{}.html", anime.key, episode.key)
		});

		let is_ngc = stream.key.eq_ignore_ascii_case("NGC");
		let (url, headers, stream_type) = if is_ngc {
			let (u, h) = resolve_ngc(&base, &watch_url, &fid)?;
			(u, h, StreamType::HLS)
		} else {
			let iframe = watch_iframe_url(&base, &watch_url, &fid, 0)?;
			let (u, h) = resolve_pai(&iframe)?;
			(u, h, StreamType::HLS)
		};

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

impl ListingProvider for NguonphimeSource {
	fn get_anime_list(&self, listing: Listing, page: i32) -> Result<AnimePageResult> {
		if !matches!(listing.kind, ListingKind::List) {
			return Ok(AnimePageResult::default());
		}
		let base = self.base();
		// Listing ids ARE the site's relative list paths.
		let url = if listing.id.contains('?') {
			format!("{base}/{}&page={page}", listing.id)
		} else {
			format!("{base}/{}?page={page}", listing.id)
		};
		let (entries, has_next_page) = self.page(&url, page)?;
		Ok(AnimePageResult {
			entries,
			has_next_page,
		})
	}
}

impl DynamicListings for NguonphimeSource {
	fn get_dynamic_listings(&self) -> Result<Vec<Listing>> {
		Ok(LISTINGS
			.iter()
			.map(|(id, name)| Listing {
				id: (*id).to_string(),
				name: (*name).to_string(),
				kind: ListingKind::List,
			})
			.collect())
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Home
// ────────────────────────────────────────────────────────────────────────────

impl Home for NguonphimeSource {
	fn get_home(&self) -> Result<HomeLayout> {
		let base = self.base();
		let urls = [
			format!("{base}/tuy-chon/phim-moi.html?ft=ne&ne=1&page=1"),
			format!("{base}/tuy-chon/phim-bo.html?ft=ty&ty=2&page=1"),
			format!("{base}/tuy-chon/phim-le.html?ft=ty&ty=1&page=1"),
		];

		// 3 parallel requests for the home rails (checker-bounce tolerated).
		let mut requests = Vec::with_capacity(3);
		for u in &urls {
			requests.push(Request::get(u.as_str())?);
		}
		let responses = Request::send_all(requests);

		let mut pages: [Vec<Anime>; 3] = [Vec::new(), Vec::new(), Vec::new()];
		for (i, resp) in responses.into_iter().enumerate() {
			let Ok(resp) = resp else { continue };
			let Ok(body) = resp.get_string() else { continue };
			let body = if is_checker_page(&body) {
				// Retry after the checker hop set session cookies.
				match Request::get(urls[i.min(2)].as_str()) {
					Ok(req) => match req.string() {
						Ok(b) => b,
						Err(_) => continue,
					},
					Err(_) => continue,
				}
			} else {
				body
			};
			let Ok(doc) = Html::parse(body) else { continue };
			let (cards, _) = parse_list_page(&doc, &base, 1);
			pages[i.min(2)] = cards;
		}

		let (latest, bo, le) = (pages[0].clone(), pages[1].clone(), pages[2].clone());
		let mut components: Vec<HomeComponent> = Vec::new();

		// Featured (banner) — top 5 latest.
		if !latest.is_empty() {
			let links: Vec<Link> = latest
				.iter()
				.take(5)
				.map(|m| Link {
					title: m.title.clone(),
					image_url: Some(m.cover.clone()),
					value: Some(LinkValue::Anime(m.clone())),
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

			// Recently updated (episode badge).
			let listed: Vec<AnimeWithEpisode> = latest
				.iter()
				.take(8)
				.map(|m| AnimeWithEpisode {
					anime: m.clone(),
					episode: home_episode(m),
				})
				.collect();
			components.push(HomeComponent {
				title: Some(String::from("Mới Cập Nhật")),
				value: HomeComponentValue::AnimeEpisodeList {
					page_size: None,
					entries: listed,
					listing: Some(Listing {
						id: String::from("tuy-chon/phim-moi.html?ft=ne&ne=1"),
						name: String::from("Mới Cập Nhật"),
						kind: ListingKind::List,
					}),
				},
				..Default::default()
			});
		}

		// Series (site category "Phim Bộ").
		if !bo.is_empty() {
			let entries: Vec<Link> = bo
				.iter()
				.take(10)
				.map(|m| Link {
					title: m.title.clone(),
					subtitle: m.current_episode.clone(),
					image_url: Some(m.cover.clone()),
					value: Some(LinkValue::Anime(m.clone())),
				})
				.collect();
			components.push(HomeComponent {
				title: Some(String::from("Phim Bộ")),
				value: HomeComponentValue::Scroller {
					entries,
					listing: Some(Listing {
						id: String::from("tuy-chon/phim-bo.html?ft=ty&ty=2"),
						name: String::from("Phim Bộ"),
						kind: ListingKind::List,
					}),
				},
				..Default::default()
			});
		}

		// Single movies (site category "Phim Lẻ").
		if !le.is_empty() {
			let entries: Vec<Link> = le
				.iter()
				.take(8)
				.map(|m| Link {
					title: m.title.clone(),
					subtitle: m.current_episode.clone(),
					image_url: Some(m.cover.clone()),
					value: Some(LinkValue::Anime(m.clone())),
				})
				.collect();
			components.push(HomeComponent {
				title: Some(String::from("Phim Lẻ")),
				value: HomeComponentValue::AnimeList {
					ranking: false,
					page_size: None,
					entries,
					listing: Some(Listing {
						id: String::from("tuy-chon/phim-le.html?ft=ty&ty=1"),
						name: String::from("Phim Lẻ"),
						kind: ListingKind::List,
					}),
				},
				..Default::default()
			});
		}

		// Genre chips → search with the "genre" filter.
		let genre_items: Vec<FilterItem> = GENRES
			.iter()
			.map(|(name, _, _)| FilterItem {
				title: (*name).to_string(),
				values: Some(vec![FilterValue::MultiSelect {
					id: String::from("genre"),
					included: vec![(*name).to_string()],
					excluded: Vec::new(),
				}]),
			})
			.collect();
		components.push(HomeComponent {
			title: Some(String::from("Thể Loại")),
			value: HomeComponentValue::Filters(genre_items),
			..Default::default()
		});

		// Quick links to the listings.
		let links: Vec<Link> = LISTINGS
			.iter()
			.map(|(id, name)| Link {
				title: (*name).to_string(),
				value: Some(LinkValue::Listing(Listing {
					id: (*id).to_string(),
					name: (*name).to_string(),
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
// Filters & settings
// ────────────────────────────────────────────────────────────────────────────

impl DynamicFilters for NguonphimeSource {
	fn get_dynamic_filters(&self) -> Result<Vec<Filter>> {
		Ok(vec![
			SelectFilter {
				id: "type".into(),
				title: Some("Loại phim".into()),
				options: {
					let mut o: Vec<Cow<'static, str>> = vec!["Tất cả".into()];
					o.extend(TYPE_FILTERS.iter().map(|(n, ..)| Cow::Borrowed(*n)));
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
				options: GENRES.iter().map(|(n, _, _)| Cow::Borrowed(*n)).collect(),
				..Default::default()
			}
			.into(),
			MultiSelectFilter {
				id: "country".into(),
				title: Some("Quốc gia".into()),
				is_genre: false,
				uses_tag_style: true,
				options: COUNTRIES.iter().map(|(n, _, _)| Cow::Borrowed(*n)).collect(),
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
				"Nguồn dữ liệu từ Nguồn Phim (https://nguonphime.site). Mỗi trang phim có hai máy phát: PAI (HLS trực tiếp) và NGC.",
			),
		])
	}
}

impl DynamicSettings for NguonphimeSource {
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		Ok(vec![
			TextSetting {
				key: SETTING_BASE_URL.into(),
				title: "Địa chỉ trang Nguồn Phim".into(),
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

impl NotificationHandler for NguonphimeSource {
	fn handle_notification(&self, notification: String) {
		// No meaningful cache; just record the change (base_url is read
		// directly via defaults_get on every request).
		defaults_set(
			SETTING_LAST_NOTIFICATION,
			DefaultValue::String(notification),
		);
	}
}

// ────────────────────────────────────────────────────────────────────────────
// Deep links & migration — keys are the site paths, identity-stable.
// ────────────────────────────────────────────────────────────────────────────

impl DeepLinkHandler for NguonphimeSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		if let Some(i) = url.find("/xem-phim/") {
			let tail = &url[i + "/xem-phim/".len()..];
			if let Some(seg) = tail.split('/').next()
				&& let Some(key) = film_key_prefix(seg)
			{
				return Ok(Some(DeepLinkResult::Anime { key }));
			}
		}
		let seg = url
			.trim_end_matches('/')
			.rsplit('/')
			.next()
			.unwrap_or(&url);
		if let Some(key) = film_key_prefix(seg) {
			return Ok(Some(DeepLinkResult::Anime { key }));
		}
		Ok(None)
	}
}

impl MigrationHandler for NguonphimeSource {
	fn handle_anime_migration(&self, key: String) -> Result<String> {
		Ok(key)
	}

	fn handle_episode_migration(&self, _anime_key: String, episode_key: String) -> Result<String> {
		Ok(episode_key)
	}
}

register_source!(
	NguonphimeSource,
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

	/// A real grab-page playlist `[{file,…}]` encoded exactly as the site does
	/// (`var v<hex> = "<base64>"`, `JSON.parse(atob(…))`).
	const GRAB_HTML: &str = r#"<!DOCTYPE html><html><head><script type="text/javascript">
var jwplayer = {};
var v17900506216ab2013d1c8a1 = "W3siZmlsZSI6Imh0dHBzOi8vYS5rdnA3MjYuY29tLzIwMjYwOTIxL21pZXBhQ3RVL2luZGV4Lm0zdTgiLCJsYWJlbCI6IjAiLCJ0eXBlIjoiaGxzIiwiZGVmYXVsdCI6dHJ1ZX1d";
var v17900506216ab2013d1c8a2 = "W3siZmlsZSI6Imh0dHBzOi8vdG9rLmNkbi9rZXkvaW5kZXgubTN1OCIsInN0cmVhbVVybCI6Imh0dHBzOi8vbmd1b25zdHJlYW0udG9wL2tleSIsInRva2VuIjoidGsxMjMiLCJsYWJlbCI6IjAiLCJ0eXBlIjoiaGxzIiwiZGVmYXVsdCI6dHJ1ZX1d";
var url = '';
</script></head><body>
<ul class="listServer"><li class="serverItem" data-index="0">PAI</li><li class="serverItem" data-index="1">NGC</li></ul>
<script type="text/javascript">
jQuery('.serverItem').on('click', function () {
    npPhim.setIndexL(jQuery(this).attr('data-index'));
    npPhim.getPlayerAgain();
});
npPhim.getPlayerAgain = function () {
    var url = '/xem-phim/lan-huong-nhu-co-against-the-current-f83892-24-e1007951.html?key=rFOkmaSeV2tiZ2lnaWphamNkY1uqpJaij6CdV2thrQ&tim=1790050934&fromEmbed=1&api=nguonphime.site';
};
</script></body></html>"#;

	/// One home-grid card.
	const CARD_HTML: &str = r#"<div class="item-file-index border-item clearfix">
	<div class="img-item-file-index">
		<a href="/dao-hai-tac-one-piece-f47872.html" title="Đảo Hải Tặc - One Piece">
			<img class="hover-img" src="https://nps3.nguon360.com/static/media/images/film/newcover/2021/6/s350_700/vua-hai-tac-1624252456.jpg" alt="Đảo Hải Tặc - One Piece">
		</a>
		<p class="episode"><span class="current-episode">1179</span><span class="separated">/</span><span class="total-episode">10000</span></p>
	</div>
	<div class="info-item-file-index">
		<h3><a href="/dao-hai-tac-one-piece-f47872.html" title="Đảo Hải Tặc - One Piece">Đảo Hải Tặc - One Piece</a></h3>
		<div class="description"><p>
			<span><i class="fa fa-globe"></i><a href="/tuy-chon/nhat-ban.html?ft=co&co=JP" title="Nhật Bản">JP</a></span>
			<span><i class="fa fa-clock-o"></i><a href="/tuy-chon/1999.html?ft=ye&ye=1999" title="1999">1999</a></span>
			<span><i class="fa fa-eye"></i>17.908 </span>
		</p></div>
	</div>
</div>"#;

	/// A detail page skeleton (metadata rows + description).
	const DETAIL_HTML: &str = r#"<h1 class="title-2">Lan Hương Như Cố</h1>
<p class="subname">Against The Current</p>
<div class="detail-movie"><div class="left-detail">
	<div class="header-movie"><div class="img-movie height-standard">
		<img src="https://nps3.nguon360.com/static/media/images/film/newcover/2026/9/s350_700/lan-huong-nhu-co-against-the-current-1789149807.jpg" alt="Lan Hương Như Cố">
	</div></div>
	<div class="caption-movie">
		<div class="infor-movie">
			<p>Điểm                        : 0.0</p>
			<p>Đạo diễn : <a href="/tuy-chon/hoang-dinh-tuong.html?ft=di&di=37059" title="Hoàng Dĩnh Tương">Hoàng Dĩnh Tương</a> </p>
			<p>Quốc gia : <a href="/tuy-chon/trung-quoc.html?ft=co&co=CN" title="Trung Quốc">Trung Quốc</a> </p>
			<p>Thể loại : <a href="/phim-tam-ly-tinh-cam-c5.html" title="Phim Tâm Lý - Tình Cảm">Phim Tâm Lý - Tình Cảm</a>, <a href="/phim-co-trang-c14.html" title="Phim Cổ Trang">Phim Cổ Trang</a> </p>
			<p>Năm sản xuất: <a href="/tuy-chon/2026.html?ft=ye&ye=2026" title="2026">2026</a> </p>
			<p>Đang phát: 24 / 47 Tập</p>
		</div>
	</div>
</div>
<div class="film-desc"><div class="detail-film-desc">Thẩm Gia Lan, trưởng tôn nữ…</div></div>"#;

	/// A watch page with episode links (anchor text = episode number).
	const WATCH_HTML: &str = r#"<div class="listTap"><ul>
	<li><a id="eid1006847" class="episodeLink" href="/xem-phim/lan-huong-nhu-co-against-the-current-f83892-1-e1006847.html">1</a></li>
	<li><a id="eid1006848" class="episodeLink" href="/xem-phim/lan-huong-nhu-co-against-the-current-f83892-2-e1006848.html">2</a></li>
	<li><a id="eid1007951" class="episodeLink" href="/xem-phim/lan-huong-nhu-co-against-the-current-f83892-24-e1007951.html">24</a></li>
</ul></div>"#;

	/// The NP Checker interstitial body.
	const CHECKER_HTML: &str = r#"<!DOCTYPE html><html><head><title>NP Checker</title></head>
<body><script type="text/javascript">
setTimeout(function() { window.location.href = "https://nguonphime.site/"; },1000);
</script>
Chào mừng bạn đến với chúng tôi, chúc bạn luôn xem phim vui vẻ nhé! Xin vui lòng chờ trong giây lát để chuyển trang!</body></html>"#;

	/// The live-search dropdown html.
	const SEARCH_HTML: &str = r#"<div class="result-group">Phim</div>
<div class="result border-bottom"><ul>
	<li class="result-item">
		<a href="/huong-moc-lan-magnolia-f38120.html" title="Hương Mộc Lan">
			<div class="result-item-box clearfix">
				<div class="result-item-image"><img src="https://nps3.nguon360.com/static/media/images/film/vp/s100_200/huong-moc-lan-1589588429.jpg" alt="Hương Mộc Lan"/></div>
				<div class="result-item-content">
					<p class="result-item-title">Hương Mộc Lan</p>
					<p class="result-item-title result-item-title-en">Magnolia</p>
				</div>
			</div>
		</a>
	</li>
</ul></div>"#;

	#[komorei_test]
	fn b64_decode_roundtrips_standard_and_padded() {
		assert_eq!(b64_decode("TWFu").as_deref(), Some(&b"Man"[..]));
		assert_eq!(b64_decode("TWE=").as_deref(), Some(&b"Ma"[..]));
		assert_eq!(b64_decode("TQ==").as_deref(), Some(&b"M"[..]));
		assert_eq!(b64_decode("SGVsbG8gd29ybGQh").as_deref(), Some(&b"Hello world!"[..]));
		assert_eq!(b64_decode("aGVsbG8=").as_deref(), Some(&b"hello"[..]));
		// invalid chars / bad length
		assert!(b64_decode("he!!o").is_none());
		assert!(b64_decode("TQ=").is_none());
	}

	#[komorei_test]
	fn extracts_playlist_from_grab_page() {
		let entries = extract_playlist(GRAB_HTML).expect("grab playlist decodes");
		assert_eq!(entries.len(), 1);
		assert_eq!(entries[0].file, "https://a.kvp726.com/20260921/miepaCtU/index.m3u8");
		assert_eq!(entries[0].kind.as_deref(), Some("hls"));
		assert_eq!(entries[0].default, Some(true));
		assert!(entries[0].token.is_none());
	}

	#[komorei_test]
	fn playlist_entry_carries_token_and_stream_url() {
		let token_html = r#"var vx = "W3siZmlsZSI6Imh0dHBzOi8vdG9rLmNkbi9rZXkvaW5kZXgubTN1OCIsInN0cmVhbVVybCI6Imh0dHBzOi8vbmd1b25zdHJlYW0udG9wL2tleSIsInRva2VuIjoidGsxMjMiLCJsYWJlbCI6IjAiLCJ0eXBlIjoiaGxzIiwiZGVmYXVsdCI6dHJ1ZX1d";"#;
		let entries = extract_playlist(token_html).expect("token playlist decodes");
		assert_eq!(entries[0].token.as_deref(), Some("tk123"));
		assert_eq!(
			entries[0].stream_url.as_deref(),
			Some("https://nguonstream.top/key")
		);
	}

	#[komorei_test]
	fn cards_parse_to_lite_anime() {
		let doc = Html::parse(CARD_HTML).expect("card html parses");
		let (cards, has_next) = parse_list_page(&doc, DEFAULT_BASE, 1);
		assert!(!has_next); // no Pager in the fixture
		assert_eq!(cards.len(), 1);
		let c = &cards[0];
		assert_eq!(c.key, "dao-hai-tac-one-piece-f47872");
		assert_eq!(c.title, "Đảo Hải Tặc - One Piece");
		assert_eq!(c.current_episode.as_deref(), Some("Tập 1179"));
		assert_eq!(c.episode_count, 10000);
		assert_eq!(c.release_year.as_ref().map(|y| y.name.as_str()), Some("1999"));
		assert_eq!(c.countries.first().map(|x| x.name.as_str()), Some("Nhật Bản"));
		assert_eq!(
			c.cover.as_str(),
			"https://nps3.nguon360.com/static/media/images/film/newcover/2021/6/s350_700/vua-hai-tac-1624252456.jpg"
		);
		assert_eq!(
			c.url.as_deref(),
			Some("https://nguonphime.site/dao-hai-tac-one-piece-f47872.html")
		);
	}

	#[komorei_test]
	fn detail_page_parses_metadata_and_status() {
		let doc = Html::parse(DETAIL_HTML).expect("detail parses");
		let info = parse_detail(&doc);
		assert_eq!(info.title, "Lan Hương Như Cố");
		assert_eq!(info.subname, "Against The Current");
		assert!(info.cover.as_deref().unwrap().contains("lan-huong-nhu-co"));
		assert_eq!(info.description, "Thẩm Gia Lan, trưởng tôn nữ…");
		assert_eq!(info.score, Some(0.0));
		assert_eq!(info.directors, vec!["Hoàng Dĩnh Tương"]);
		assert_eq!(info.genres, vec!["Phim Tâm Lý - Tình Cảm", "Phim Cổ Trang"]);
		assert_eq!(info.countries, vec!["Trung Quốc"]);
		assert_eq!(info.year.as_deref(), Some("2026"));
		assert_eq!(info.current_episode, Some(24));
		assert_eq!(info.total_episodes, Some(47));

		let full = build_full(DEFAULT_BASE, "lan-huong-nhu-co-against-the-current-f83892", info);
		assert_eq!(full.status, AnimeStatus::Ongoing);
		assert_eq!(full.episode_count, 47);
		assert_eq!(full.rating, Some(0.0));
		assert_eq!(full.genres.len(), 2);
		assert_eq!(full.current_episode.as_deref(), Some("Tập 24"));
		assert_eq!(full.seasons.len(), 2);
		assert_eq!(full.seasons[0].anime_id, "lan-huong-nhu-co-against-the-current-f83892|PAI");
		assert_eq!(full.seasons[1].title, "NGC");
	}

	#[komorei_test]
	fn completed_series_when_current_equals_total() {
		let html = DETAIL_HTML.replacen("24 / 47 Tập", "14 / 14 Tập", 1);
		let doc = Html::parse(html).expect("detail parses");
		let info = parse_detail(&doc);
		assert_eq!(build_full(DEFAULT_BASE, "x-f125", info).status, AnimeStatus::Completed);
	}

	#[komorei_test]
	fn watch_page_parses_full_episode_list() {
		let doc = Html::parse(WATCH_HTML).expect("watch parses");
		let eps = parse_episodes(&doc, DEFAULT_BASE, "lan-huong-nhu-co-against-the-current-f83892");
		assert_eq!(eps.len(), 3);
		assert_eq!(eps[0].key, "1-e1006847");
		assert_eq!(eps[0].episode_number, "1");
		assert_eq!(
			eps[0].url.as_deref(),
			Some("https://nguonphime.site/xem-phim/lan-huong-nhu-co-against-the-current-f83892-1-e1006847.html")
		);
		assert_eq!(eps[2].episode_number, "24");
	}

	#[komorei_test]
	fn checker_page_is_detected() {
		assert!(is_checker_page(CHECKER_HTML));
		assert!(is_checker_page("<title>NP Checker</title>"));
		assert!(!is_checker_page(CARD_HTML));
		assert!(!is_checker_page(DETAIL_HTML));
	}

	#[komorei_test]
	fn search_items_parse_from_dropdown() {
		let doc = Html::parse(SEARCH_HTML).expect("search parses");
		let items = parse_search_items(&doc, DEFAULT_BASE);
		assert_eq!(items.len(), 1);
		assert_eq!(items[0].key, "huong-moc-lan-magnolia-f38120");
		assert_eq!(items[0].title, "Hương Mộc Lan");
		assert_eq!(items[0].original_title, "Magnolia");
		assert!(items[0].cover.contains("huong-moc-lan-1589588429.jpg"));
	}

	#[komorei_test]
	fn from_embed_url_is_extracted() {
		let path = from_embed_url(GRAB_HTML).expect("fromEmbed url found");
		assert!(path.starts_with("/xem-phim/"));
		assert!(path.contains("fromEmbed=1"));
		assert_eq!(query_param(&path, "tim").as_deref(), Some("1790050934"));
	}

	#[komorei_test]
	fn build_list_urls_follow_first_selected_filter() {
		assert_eq!(
			build_list_url(DEFAULT_BASE, 2, &[]),
			"https://nguonphime.site/tuy-chon/phim-moi.html?ft=ne&ne=1&page=2"
		);
		let type_f = vec![FilterValue::Select {
			id: "type".into(),
			value: "Phim Bộ".into(),
		}];
		assert_eq!(
			build_list_url(DEFAULT_BASE, 1, &type_f),
			"https://nguonphime.site/tuy-chon/phim-bo.html?ft=ty&ty=2&page=1"
		);
		let genre_f = vec![FilterValue::MultiSelect {
			id: "genre".into(),
			included: vec!["Kinh Dị".into()],
			excluded: Vec::new(),
		}];
		assert_eq!(
			build_list_url(DEFAULT_BASE, 3, &genre_f),
			"https://nguonphime.site/phim-kinh-di-c9.html?page=3"
		);
		let country_f = vec![FilterValue::MultiSelect {
			id: "country".into(),
			included: vec!["Hàn Quốc".into()],
			excluded: Vec::new(),
		}];
		assert_eq!(
			build_list_url(DEFAULT_BASE, 1, &country_f),
			"https://nguonphime.site/tuy-chon/han-quoc.html?ft=co&co=KR&page=1"
		);
		let year_f = vec![FilterValue::Select {
			id: "year".into(),
			value: "2026".into(),
		}];
		assert_eq!(
			build_list_url(DEFAULT_BASE, 1, &year_f),
			"https://nguonphime.site/tuy-chon/2026.html?ft=ye&ye=2026&page=1"
		);
	}

	#[komorei_test]
	fn film_id_and_key_prefix_parse_from_paths() {
		assert_eq!(
			film_id("lan-huong-nhu-co-against-the-current-f83892").as_deref(),
			Some("83892")
		);
		assert_eq!(film_id("melody-f1").as_deref(), Some("1"));
		assert_eq!(film_id("no-id-here").as_deref(), None);

		assert_eq!(
			film_key_prefix("lan-huong-nhu-co-against-the-current-f83892-24-e1007951.html").as_deref(),
			Some("lan-huong-nhu-co-against-the-current-f83892")
		);
		assert_eq!(
			film_key_prefix("dao-hai-tac-one-piece-f47872.html").as_deref(),
			Some("dao-hai-tac-one-piece-f47872")
		);
	}

	#[komorei_test]
	fn seconds_time_is_available_for_the_watch_post() {
		// current_date() is the wasm std-host clock; on the test host it is
		// a reasonable unix timestamp.
		let t = current_date();
		assert!(t > 1_500_000_000);
		assert!(t.to_string().len() == 10);
	}
}