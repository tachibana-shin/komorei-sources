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
//! [`net::fetch_html`] detects the checker body (`<title>NP Checker</title>` /
//! the "Chào mừng…" greeting) and simply repeats the request once — the
//! cookies are already stored by the time of the retry. Everything else needs
//! no special handling (verified with plain curl: the grab embed pages answer
//! without cookies too).
//!
//! ## Layout
//!
//! - [`catalog`] — site constants, genre/country/type catalogs, listings.
//! - [`models`] — JSON envelopes (`WatchPostResp`, streamc grants, the
//!   obfuscated playlist entry) + the parsed detail-page bag.
//! - [`util`] — pure string/number/URL helpers.
//! - [`parsers`] — the jsoup parsing layer (cards, detail, episodes, search,
//!   grab playlist, list-url builder).
//! - [`net`] — fetch + the two playback-resolver chains (PAI / NGC).
//! - [`home`] — the `Home` layout.
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
//!      `JSON.parse(atob(…))`) → decode (the runner's native `imports::base64`)
//!      to `[{file, label, type, default, token, streamUrl}]`. **PAI**
//!      (`indexL=0`) yields a direct HLS file on `a.kvp726.com` with no
//!      required headers. **NGC** (`indexL=1`) is a second XHR to the page's
//!      own `var url = '…fromEmbed=1&api=…'` path → returns an iframe to
//!      `embed{N}.streamc.xyz/embed.php?hash=…` — the shared streamc grant
//!      engine (bootstrap → issue) also used by
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

mod catalog;
mod home;
mod models;
mod net;
mod parsers;
mod util;

#[cfg(test)]
mod tests;

use alloc::{
	borrow::Cow,
	format,
	string::{String, ToString},
	vec,
	vec::Vec,
};
use komorei::{
	Anime, AnimePageResult, ButtonSetting, DeepLinkHandler, DeepLinkResult, DynamicFilters,
	DynamicListings, DynamicSettings, Episode, Filter, FilterValue, Listing, ListingKind,
	ListingProvider, MigrationHandler, MultiSelectFilter, NotificationHandler, Result,
	SelectFilter, Setting, Source, StreamData, StreamInfo, StreamType, TextSetting,
	helpers::uri::encode_uri_component,
	imports::defaults::{DefaultValue, defaults_get, defaults_set},
	imports::html::Html,
	prelude::*,
};

use crate::catalog::{
	COUNTRIES, DEFAULT_BASE, GENRES, LISTINGS, SERVERS, SETTING_BASE_URL,
	SETTING_LAST_NOTIFICATION, TYPE_FILTERS, year_options,
};
use crate::models::DetailInfo;
use crate::net::{fetch_html, resolve_ngc, resolve_pai, watch_iframe_url, watch_post};
use crate::parsers::{
	build_full, build_list_url, parse_detail, parse_episodes, parse_list_page, parse_search_items,
};
use crate::util::{film_id, film_key_prefix};

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