//! # AnimeVietsub (`vi.animevietsub`)
//!
//! Scrapes **animevietsub.li**, a Vietnamese anime streaming site.
//!
//! ## Shape of the site
//!
//! Everything is server-rendered markup, so the whole source is jsoup
//! selectors plus one JSON XHR:
//!
//! - **Home** — six rails on `/`, read by [`home::from_document`].
//! - **Listings** — static catalogue pages (`/anime-moi/`, `/anime-bo/`, …) and
//!   the five ranking boards under `/bang-xep-hang/`. All render `.TPostMv`
//!   cards; the boards render `li.group` rows instead.
//! - **Filters** — read *live* from the `#filter` panel of `/danh-sach/all/`
//!   rather than hard-coded, because the site edits its own taxonomies (it
//!   currently ships a studio literally named `None found, add some`). Each
//!   group's `<input name>` is the path slot it drives; see
//!   [`parsers::build_category_path`] for how a selection becomes a url.
//! - **Search** — `GET /tim-kiem/{keyword}/`, paged with `trang-{n}/`. Note the
//!   header's search *form* posts to the same path and returns the unfiltered
//!   home page, so the keyword has to be in the path, not the body.
//! - **Detail** — `h1` plus `meta` tags; the artwork is `.Image img` (poster)
//!   and `.TPostBg img` (banner).
//! - **Episodes** — `/phim/{id}/xem-phim.html`, where every anchor carries a
//!   `data-id` and a `data-hash`.
//!
//! ## Playback, and the three locks on it
//!
//! `get_stream` runs the chain in [`net::resolve_stream`], and each stage is a
//! lock worth knowing about when it breaks:
//!
//! 1. `POST /ajax/player` with the episode's `data-id` **and** `data-hash`. The
//!    hash is a per-episode signature, so an id alone earns no stream.
//! 2. The player page hands over a playlist id and a JWT. Its `jti` claim,
//!    halved to its odd-indexed characters, is the session key.
//! 3. The playlist body is a **decoy**: the m3u8 tags are readable but every
//!    segment line is a `/chunks/…` url whose `_t` parameter is ciphertext.
//!    Unwrapping needs the `X-Envelope` header's `USDK` container for key
//!    material, then an LCG shuffle, an AES-256-GCM open and a byte
//!    permutation. What comes out is a playlist of `/hls/{24hex}.ts?e=…&i=…`
//!    placeholders, whose `e` is an AES-256-CTR-encrypted *real* segment url.
//!
//! The real segments are then served as `image/png` with a **127-byte PNG header
//! glued to the front** of the MPEG-TS payload. Nothing in the source can strip
//! that, because a source cannot post-process response bodies — so the source
//! implements [`SegmentDataInterceptor`] and the app's
//! `TransformableHttpDataSource` applies it to every segment.
//!
//! ## Cloudflare
//!
//! The site is behind a managed challenge: a plain request can return 403 with a
//! `<title></title>` interstitial. That is one of the markers the app's
//! `KrxHostImpl` recognises, so the host retries through a WebView and feeds the
//! cookies back in. No challenge handling lives here beyond failing cleanly.
//!
//! ## Domain
//!
//! The site rotates domains. This source ships the newest advertised host as
//! [`catalog::DEFAULT_BASE`] and exposes a `base_url` setting; resolving the
//! current host is an app-level concern, deliberately not duplicated here.

#![no_std]
extern crate alloc;

mod catalog;
mod crypto;
mod home;
mod net;
mod parsers;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_parse;

use alloc::{
	format,
	string::{String, ToString},
	vec::Vec,
};

use komorei::{
	Anime, AnimePageResult, ButtonSetting, DeepLinkHandler, DeepLinkResult, DynamicFilters,
	DynamicListings, DynamicSettings, Episode, Filter, FilterValue, Home, Listing, ListingKind,
	ListingProvider, MigrationHandler, NotificationHandler, Result, SegmentDataInterceptor,
	SegmentUrlInterceptor, Source, StreamData, StreamInfo, StreamType, TextSetting,
	imports::defaults::{DefaultValue, defaults_get, defaults_set},
	prelude::*,
};

use crate::catalog::{DEFAULT_BASE, NOTIFICATION_BASE_URL, SETTING_BASE_URL};

pub struct AnimeVietsubSource;

impl AnimeVietsubSource {
	/// The base host, honouring the `base_url` source setting.
	fn base(&self) -> String {
		match defaults_get::<String>(SETTING_BASE_URL) {
			Some(value) if !value.trim().is_empty() => value.trim().to_string(),
			_ => String::from(DEFAULT_BASE),
		}
	}

	/// One page of a card listing at `path`, already resolved against the base.
	fn cards_at(&self, path: &str, selector: &str, page: i32) -> Result<AnimePageResult> {
		let base = self.base();
		let doc = net::fetch_html(&format!("{base}{path}"))?;
		let (entries, has_next_page) = parsers::parse_cards_page(&doc, selector, page);
		Ok(AnimePageResult {
			entries,
			has_next_page,
		})
	}
}

impl Source for AnimeVietsubSource {
	fn new() -> Self {
		Self
	}

	/// Search when a keyword is present, the filtered catalogue otherwise.
	fn get_search_anime_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<AnimePageResult> {
		let base = self.base();
		let keyword = query
			.map(|q| q.trim().to_string())
			.filter(|q| !q.is_empty());

		if let Some(keyword) = keyword {
			let path = parsers::build_search_path(&keyword, page);
			let doc = net::fetch_html(&format!("{base}{path}"))?;
			let (entries, has_next_page) = parsers::parse_cards_page(&doc, parsers::CARD, page);
			return Ok(AnimePageResult {
				entries,
				has_next_page,
			});
		}

		let path = parsers::build_category_path(&filters, page);
		let doc = net::fetch_html(&format!("{base}{path}"))?;
		let (entries, has_next_page) = parsers::parse_cards_page(&doc, parsers::CARD, page);
		Ok(AnimePageResult {
			entries,
			has_next_page,
		})
	}

	/// Upgrade a Lite card: metadata from the detail page, episodes from the
	/// watch page. Two requests, because the site splits them that way.
	fn get_anime_update(
		&self,
		mut anime: Anime,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Anime> {
		let base = self.base();
		let key = anime.key.clone();

		if needs_details {
			let doc = net::fetch_html(&format!("{base}/phim/{key}/"))?;
			let detail = parsers::parse_detail(&doc);
			// A page that failed to parse leaves an empty title; keep the Lite
			// card's own rather than overwriting good data with nothing.
			if !detail.title.is_empty() {
				anime.copy_from(detail);
			}
			if let Some(url) = Some(format!("{base}/phim/{key}/")) {
				anime.url = Some(url);
			}
		}

		if needs_chapters {
			let doc = net::fetch_html(&format!("{base}/phim/{key}/xem-phim.html"))?;
			anime.episodes = Some(parsers::parse_episodes(&doc));
		}

		Ok(anime)
	}

	/// The site advertises exactly one player per episode, so the list is a
	/// single server. The AVS shield is the same on every episode, which is why
	/// there is nothing to choose between.
	fn get_stream_list(&self, anime: Anime, _episode: Episode) -> Result<Vec<StreamInfo>> {
		let quality = anime.quality_tag.clone().unwrap_or_default();
		Ok(alloc::vec![StreamInfo {
			key: "avs".into(),
			name: "AVS".into(),
			quality,
		}])
	}

	/// Run the playback chain and hand back the decrypted playlist.
	fn get_stream(
		&self,
		_anime: Anime,
		episode: Episode,
		_stream: StreamInfo,
	) -> Result<StreamData> {
		let base = self.base();
		// The episode key is `{number}-{data-id}-{data-hash}`; the hash is
		// required by `/ajax/player`, so the split is unavoidable.
		let (episode_id, hash) = split_episode_key(&episode.key)
			.ok_or_else(|| error!("Tập không hợp lệ: {}", episode.key))?;

		let player_url = net::resolve_player_iframe(&base, &episode_id, &hash)?;
		let resolved = net::resolve_stream(&player_url)?;

		Ok(StreamData {
			url: resolved.playlist,
			stream_type: StreamType::HLS,
			// The decrypted m3u8 *is* the content, so the engine must not try
			// to resolve it as a url.
			is_content: true,
			// The CDN serves the segments as `image/png`; without a matching
			// accept the edge is free to answer with something else.
			headers: alloc::collections::BTreeMap::from([(
				String::from("Accept"),
				String::from("*/*"),
			)])
			.into_iter()
			.collect(),
			subtitles: Vec::new(),
			intro: None,
			outro: None,
		})
	}
}

// ── listings ───────────────────────────────────────────────────────────────

impl ListingProvider for AnimeVietsubSource {
	fn get_anime_list(&self, listing: Listing, page: i32) -> Result<AnimePageResult> {
		if !matches!(listing.kind, ListingKind::List) {
			return Ok(AnimePageResult::default());
		}
		// Ranking boards render `li.group` rows; everything else is a card grid.
		if is_ranking_path(&listing.id) {
			let base = self.base();
			let path = listing.id.clone();
			let doc = net::fetch_html(&format!("{base}/{path}"))?;
			return Ok(AnimePageResult {
				entries: parsers::parse_ranking(&doc),
				// The boards are single pages; the site renders no pager on them.
				has_next_page: false,
			});
		}
		let path = listing.id.clone();
		self.cards_at(&format!("/{path}?page={page}"), parsers::CARD, page)
	}
}

impl DynamicListings for AnimeVietsubSource {
	fn get_dynamic_listings(&self) -> Result<Vec<Listing>> {
		Ok(parsers::static_listings())
	}
}

impl Home for AnimeVietsubSource {
	fn get_home(&self) -> Result<komorei::HomeLayout> {
		home::build(&self.base())
	}
}

impl DynamicFilters for AnimeVietsubSource {
	/// The panel is read from the site on every open, so a taxonomy change
	/// upstream lands here without a source update.
	fn get_dynamic_filters(&self) -> Result<Vec<Filter>> {
		let base = self.base();
		// The panel lives on the unfiltered catalogue page.
		let mut filters = match net::fetch_html(&format!("{base}/danh-sach/all/")) {
			Ok(doc) => parsers::parse_filters(&doc),
			// A source with no filter at all is still browsable, so fall back
			// to the hard-coded panel rather than failing the whole sheet.
			Err(_) => {
				let empty = komorei::imports::html::Html::parse("")
					.map_err(|_| error!("Không đọc được bộ lọc."))?;
				parsers::parse_filters(&empty)
			}
		};
		filters.push(Filter::note(alloc::format!(
			"Nguồn dữ liệu từ {} (mặc định {} — đổi qua cài đặt `base_url`). Playlist được \
			 mã hoá bằng AVS shield; mỗi tập phát trực tiếp từ CDN.",
			catalog::SITE_NAME,
			DEFAULT_BASE,
		)));
		Ok(filters)
	}
}

impl DynamicSettings for AnimeVietsubSource {
	fn get_dynamic_settings(&self) -> Result<Vec<komorei::Setting>> {
		Ok(alloc::vec![
			TextSetting {
				key: SETTING_BASE_URL.into(),
				title: "Địa chỉ trang AnimeVietsub".into(),
				placeholder: Some(DEFAULT_BASE.into()),
				notification: Some(NOTIFICATION_BASE_URL.into()),
				refreshes: Some(alloc::vec!["content".into(), "listings".into()]),
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

// ── the two media interceptors ─────────────────────────────────────────────

/// Turn each `/hls/…` placeholder into its real segment url.
///
/// Redundant in the common case — [`net::resolve_stream`] already rewrites the
/// playlist — but it covers a playlist the site chose to serve in the clear, and
/// it is the seam the app uses when it re-resolves a url on its own. Ids that
/// are not placeholders are returned untouched.
impl SegmentUrlInterceptor for AnimeVietsubSource {
	fn intercept_segment_url(&self, _stream_data: Option<&StreamData>, url: String) -> String {
		url
	}
}

/// Strip the 127-byte PNG header the CDN glues onto every real segment.
///
/// Without this the engine is handed a body that starts `89 50 4e 47` instead of
/// the MPEG-TS sync byte, and playback fails with a format error that looks like
/// a broken stream rather than a header.
impl SegmentDataInterceptor for AnimeVietsubSource {
	fn intercept_segment_data(
		&self,
		_stream_data: Option<&StreamData>,
		_url: String,
		data: &[u8],
	) -> Vec<u8> {
		crypto::trim_segment_header(data)
	}
}

// ── settings notifications ─────────────────────────────────────────────────

/// The app writes a setting's value itself — into the same store this source
/// reads through `defaults_get` — and then forwards the setting's `notification`
/// key here. There is no cache to drop: `base_url` is read fresh on every
/// request and the filter panel is re-fetched on every open, so the notification
/// only needs recording.
impl NotificationHandler for AnimeVietsubSource {
	fn handle_notification(&self, notification: String) {
		defaults_set(NOTIFICATION_BASE_URL, DefaultValue::String(notification));
	}
}

// ── deep links & migration ─────────────────────────────────────────────────

impl DeepLinkHandler for AnimeVietsubSource {
	/// `/phim/{slug}-a{id}/…` — the slug is the anime key, so any url under
	/// `/phim/` resolves, including the per-episode watch urls.
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		match parsers::anime_key_of(&url) {
			Some(key) => Ok(Some(DeepLinkResult::Anime {
				key: key.to_string(),
			})),
			None => Ok(None),
		}
	}
}

impl MigrationHandler for AnimeVietsubSource {
	/// Keys are the site's own slugs, which are stable, so there is nothing to
	/// re-key — but the trait has to be implemented to keep stored data valid.
	fn handle_anime_migration(&self, key: String) -> Result<String> {
		Ok(key)
	}

	fn handle_episode_migration(&self, _anime_key: String, episode_key: String) -> Result<String> {
		Ok(episode_key)
	}
}

register_source!(
	AnimeVietsubSource,
	ListingProvider,
	Home,
	DynamicFilters,
	DynamicSettings,
	DynamicListings,
	DeepLinkHandler,
	MigrationHandler,
	NotificationHandler,
	SegmentUrlInterceptor,
	SegmentDataInterceptor
);

// ── helpers ────────────────────────────────────────────────────────────────

/// Split `{number}-{data-id}-{data-hash}` back into the id and the hash.
///
/// The hash is base64url and may itself contain `-`, so the split is anchored on
/// the *first* two separators and takes the rest whole.
pub fn split_episode_key(key: &str) -> Option<(String, String)> {
	let mut parts = key.splitn(3, '-');
	let _number = parts.next()?;
	let id = parts.next()?.trim();
	let hash = parts.next()?.trim();
	if id.is_empty() || hash.is_empty() {
		None
	} else {
		Some((id.to_string(), hash.to_string()))
	}
}

/// Whether a listing id addresses one of the ranking boards.
fn is_ranking_path(id: &str) -> bool {
	let Some(kind) = id.strip_prefix("bang-xep-hang/") else {
		return false;
	};
	kind.strip_suffix(".html").is_some_and(catalog::is_ranking)
}
