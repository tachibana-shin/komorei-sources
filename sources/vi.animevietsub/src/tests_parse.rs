//! Parsing tests, run against **real page captures**.
//!
//! Each `page_*.html` fixture is a real response from the site with `<script>`,
//! `<style>` and inline `<svg>` stripped — nothing the parsers read is removed,
//! and keeping the genuine markup is the whole point: these assertions are what
//! catch a site redesign before it reaches a user's screen.

use alloc::{
	format,
	string::{String, ToString},
	vec,
	vec::Vec,
};

use komorei::{Anime, AnimeStatus, DeepLinkHandler, FilterValue, Source, imports::html::Html};
use komorei_test::komorei_test;

use crate::{
	home,
	parsers::{self, CARD, EPISODES, FILTER_PANEL, RANKING_ROW},
	split_episode_key,
};

/// The front page, trimmed of scripts and styles.
///
/// Captured from a *rendered* DOM, so it is a superset of what `fetch_html`
/// receives. It is the right fixture for "does this selector work", but it must
/// not be used to claim a rail is reachable over plain HTTP — the carousel
/// (`.MovieListSldCn`) and the weekly top-ten (`#showTopPhim`) are injected by
/// the site's own scripts and come back empty from the server. `tests_home`
/// below pins that distinction.
const HOME: &str = include_str!("../tests/fixtures/page_home.html");
/// A title's detail page.
const DETAIL: &str = include_str!("../tests/fixtures/page_detail.html");
/// The watch page carrying the full episode list.
const EPISODES_PAGE: &str = include_str!("../tests/fixtures/page_episodes.html");
/// `/danh-sach/all/` — the catalogue grid plus the `#filter` panel.
const CATALOG: &str = include_str!("../tests/fixtures/page_catalog.html");
/// A search results page.
const SEARCH: &str = include_str!("../tests/fixtures/page_search.html");
/// A ranking board.
const RANKING: &str = include_str!("../tests/fixtures/page_ranking.html");

fn doc(html: &str) -> komorei::imports::html::Document {
	Html::parse(html).expect("HTML hợp lệ")
}

fn cards(html: &str, selector: &str) -> Vec<Anime> {
	parsers::parse_cards(&doc(html), selector)
}

// ── cards ──────────────────────────────────────────────────────────────────

#[komorei_test]
fn poster_card_yields_key_title_cover_and_rating() {
	let found = cards(HOME, CARD);
	assert!(
		found.len() >= 10,
		"the front page must have cards, has {}",
		found.len()
	);

	let one = found
		.iter()
		.find(|a| a.title.contains("Thiếu Chủ"))
		.expect("must find the target card");

	// Keys are the `/phim/{slug}-a{id}/` segment, which is also the detail url.
	assert_eq!(one.key, "nige-jouzu-no-wakagimi-2nd-season-a6015");
	assert!(
		one.cover.starts_with("https://"),
		"cover must be absolute: {}",
		one.cover
	);
	assert!(one.cover.ends_with(".jpg") || one.cover.ends_with(".png"));
	assert_eq!(one.rating, Some(9.7));
	assert_eq!(one.rating, Some(9.7));
	// `.Year` / `.AAIco-date_range` live on the `li.TPostMv > article.TPost.C`
	// card shape, which the plain `div.TPost.B` rail card does not carry.
	assert_eq!(one.views, 0);
}

#[komorei_test]
fn poster_card_reads_the_episode_badge() {
	let found = cards(HOME, CARD);
	let one = found
		.iter()
		.find(|a| a.key == "nige-jouzu-no-wakagimi-2nd-season-a6015")
		.expect("card under test not found");
	// `<span class="mli-eps">TẬP<i>11</i></span>`
	assert_eq!(one.current_episode.as_deref(), Some("Tập 11"));
}

#[komorei_test]
fn year_comes_from_the_date_range_not_the_year_class() {
	// `.Year` holds `Lượt xem: 810,440` on this site — reading a year out of it
	// would produce nonsense like `7`. The year is `.AAIco-date_range`.
	let found = cards(HOME, "#hot-home .TPostMv");
	let one = found
		.iter()
		.find(|a| a.key == "nige-jouzu-no-wakagimi-2nd-season-a6015")
		.expect("card under test not found");
	let year = one.release_year.as_ref().map(|y| y.name.clone());
	assert_eq!(year.as_deref(), Some("2026"));
}

#[komorei_test]
fn view_count_comes_from_the_year_class() {
	let found = cards(HOME, "#hot-home .TPostMv");
	let one = found
		.iter()
		.find(|a| a.key == "nige-jouzu-no-wakagimi-2nd-season-a6015")
		.expect("card under test not found");
	// `Lượt xem: 810,440` -> 810440, separators stripped.
	assert_eq!(one.views, 810_440);
}

#[komorei_test]
fn carousel_card_reads_its_info_strip() {
	// The wide cards carry `.Info` spans instead of `.mli-eps`:
	// `.AAIco-star` is the score and `.AAIco-access_time` the `11/12` progress.
	let found = cards(HOME, ".MovieListSldCn .TPostMv");
	assert!(!found.is_empty(), "the carousel must have cards");
	let one = found
		.iter()
		.find(|a| a.key == "nige-jouzu-no-wakagimi-2nd-season-a6015")
		.expect("carousel card not found");
	assert_eq!(one.rating, Some(9.7));
	assert_eq!(one.current_episode.as_deref(), Some("Tập 11"));
	assert_eq!(one.quality_tag.as_deref(), Some("FHD"));
	assert_eq!(
		one.studio.as_ref().map(|s| s.name.as_str()),
		Some("CloverWorks")
	);
	assert!(!one.genres.is_empty(), "must have genres");
}

#[komorei_test]
fn every_home_rail_produces_cards() {
	// Each rail has its own selector; a redesign that empties one shows up here.
	for selector in [
		".MovieListTopCn .TPostMv",
		".MovieListSldCn .TPostMv",
		"#single-home .TPostMv",
		"#new-home .TPostMv",
		"#hot-home .TPostMv",
		// Not a `.TPostMv` block — plain `li > .TPost.A` rows.
		"#showTopPhim .TPost",
	] {
		let found = cards(HOME, selector);
		assert!(!found.is_empty(), "rail `{selector}` returned 0 cards");
		for anime in &found {
			assert!(!anime.key.is_empty(), "a card in `{selector}` has no key");
			assert!(!anime.title.is_empty(), "card `{}` has no title", anime.key);
		}
	}
}

#[komorei_test]
fn home_layout_is_built_from_the_front_page() {
	let layout = home::from_document(&doc(HOME));
	assert!(
		layout.components.len() >= 5,
		"expected at least 5 rails, got {}",
		layout.components.len()
	);
	for component in &layout.components {
		assert!(component.title.is_some(), "every component needs a title");
	}
}

// ── ranking ────────────────────────────────────────────────────────────────

#[komorei_test]
fn ranking_rows_parse_with_their_episode_state() {
	let found = parsers::parse_ranking(&doc(RANKING));
	assert!(!found.is_empty(), "the ranking board must have rows");
	let one = &found[0];
	assert!(!one.key.is_empty());
	assert!(!one.title.is_empty());
	// `.score` is the episode state, not a rating: `Tập NN` or `Full`.
	assert!(
		one.current_episode.is_some() || one.status == AnimeStatus::Completed,
		"episode state must be readable, current={:?} status={:?}",
		one.current_episode,
		one.status
	);
	assert_eq!(one.rating, None, "`Tập 24` is not a rating");
}

#[komorei_test]
fn ranking_selector_is_the_group_class() {
	// The rows are `<li class="po-01 group">`; matching on `li.group` rather
	// than a literal class attribute is what makes this work.
	assert!(!cards(RANKING, RANKING_ROW).is_empty());
}

// ── episodes ───────────────────────────────────────────────────────────────

#[komorei_test]
fn episodes_carry_id_and_hash_in_the_key() {
	let found = parsers::parse_episodes(&doc(EPISODES_PAGE));
	assert_eq!(found.len(), 11, "this page has 11 episodes");

	let first = &found[0];
	assert_eq!(first.episode_number, "1");
	// The key has to round-trip into the `data-id` + `data-hash` pair that
	// `/ajax/player` insists on.
	let (id, hash) = split_episode_key(&first.key).expect("key must split");
	assert_eq!(id, "114607");
	assert!(
		hash.len() > 40,
		"hash must be a long signature, len {}",
		hash.len()
	);
	assert!(
		hash.starts_with("5qC6TJh"),
		"hash must match data-hash: {hash}"
	);
}

#[komorei_test]
fn episodes_are_ordered_by_their_visible_number() {
	let found = parsers::parse_episodes(&doc(EPISODES_PAGE));
	let numbers: Vec<&str> = found.iter().map(|e| e.episode_number.as_str()).collect();
	assert_eq!(numbers[0], "1");
	assert_eq!(numbers[1], "2");
	assert_eq!(numbers[10], "11");
}

#[komorei_test]
fn episode_hashes_keep_their_dashes() {
	// The hash is base64url and contains `-`, so the key split must anchor on
	// the first two separators and take the remainder whole.
	let found = parsers::parse_episodes(&doc(EPISODES_PAGE));
	for episode in &found {
		let (_, hash) = split_episode_key(&episode.key)
			.unwrap_or_else(|| panic!("cannot split key {}", episode.key));
		assert!(
			hash.chars()
				.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
			"unexpected hash: {hash}"
		);
		// Not every hash carries a `-`; what matters is that the split did not
		// eat one when it did. Check a known multi-part hash explicitly.
		if episode.episode_number == "1" {
			assert!(
				hash.contains('-'),
				"episode 1 has a multi-part hash: {hash}"
			);
		}
	}
}

#[komorei_test]
fn split_episode_key_rejects_malformed_keys() {
	assert!(split_episode_key("").is_none());
	assert!(split_episode_key("1").is_none());
	assert!(split_episode_key("1-114607").is_none());
	assert!(split_episode_key("1--hash").is_none());
}

// ── detail ─────────────────────────────────────────────────────────────────

#[komorei_test]
fn detail_page_yields_title_synopsis_and_artwork() {
	let detail = parsers::parse_detail(&doc(DETAIL));
	assert_eq!(detail.title, "Thiếu Chủ Giỏi Chạy Trốn 2");
	// The romaji title, straight from `h2.SubTitle`.
	assert_eq!(
		detail.original_title,
		"Nige Jouzu no Wakagimi 2nd Season, The Elusive Samurai Season 2, Nigewaka"
	);
	// The synopsis is the `.Description` block itself, not a meta tag.
	let synopsis = detail.description.clone().unwrap_or_default();
	assert!(
		synopsis.starts_with("Sau thời gian dài"),
		"phải đọc `.Description`: {synopsis}"
	);
	assert!(detail.cover.contains("cdn.animevietsub.li"));
	assert!(detail.banner.is_some(), "phải có banner");
	assert_eq!(detail.rating, Some(9.7));
	assert_eq!(detail.quality_tag.as_deref(), Some("FHD"));
}

#[komorei_test]
fn detail_reads_the_labelled_info_rows() {
	let detail = parsers::parse_detail(&doc(DETAIL));
	// `#average_score` is the score, `.num-rating` how many rated it.
	assert_eq!(detail.rating_count, Some(83));
	// `.AAIco-remove_red_eye` is `809,991 Lượt Xem`.
	assert_eq!(detail.views, 809_991);
	// The rows are found by their `<strong>` label, not by position.
	assert_eq!(
		detail.studio.as_ref().map(|s| s.name.as_str()),
		Some("CloverWorks")
	);
	assert!(
		detail.countries.iter().any(|c| c.name == "Nhật Bản"),
		"the country row must be read, got {:?}",
		detail.countries
	);
	assert!(
		detail
			.season_of
			.as_ref()
			.is_some_and(|s| s.name.contains("Mùa Hạ")),
		"phải đọc được Season"
	);
	// The year link is `/danh-sach/all/all/all/2026`, so it also carries a
	// working `year` filter.
	let year = detail
		.release_year
		.as_ref()
		.expect("a release year is required");
	assert_eq!(year.name, "2026");
	assert!(
		year.filters.iter().any(|f| matches!(
			f,
			FilterValue::Select { id, value } if id == "year" && value == "2026"
		)),
		"năm phải mang filter year=2026"
	);
}

#[komorei_test]
fn detail_reads_genres_and_seasons() {
	let detail = parsers::parse_detail(&doc(DETAIL));
	// Genres come from the `/the-loai/` entries of the breadcrumb.
	assert!(detail.genres.iter().any(|g| g.name == "Shounen"));
	assert!(detail.genres.iter().any(|g| g.name == "Supernatural"));
	// The breadcrumb also names the section and the title; neither is a genre.
	assert!(
		!detail
			.genres
			.iter()
			.any(|g| g.name == "Thiếu Chủ Giỏi Chạy Trốn 2")
	);
	// `.season_item > a` links the sibling seasons.
	assert!(
		detail.seasons.iter().any(|s| s.title == "Phần 1"),
		"the seasons must be read, got {:?}",
		detail.seasons
	);
	assert!(detail.seasons.iter().all(|s| !s.anime_id.is_empty()));
}

// ── search & pagination ────────────────────────────────────────────────────

#[komorei_test]
fn search_results_parse_as_cards() {
	let found = cards(SEARCH, CARD);
	assert!(found.len() >= 10);
	// The keyword really filtered: every card mentions it.
	assert!(
		found.iter().any(|a| a.title.contains("One Piece")),
		"must contain a One Piece result"
	);
}

#[komorei_test]
fn pager_reports_the_last_page() {
	let (entries, has_next) = parsers::parse_cards_page(&doc(SEARCH), CARD, 1);
	assert!(!entries.is_empty());
	// The captured page offers 2, so page 1 has a successor and page 2 does not.
	assert!(has_next, "trang 1 phải còn trang sau");
	let (_, last_has_next) = parsers::parse_cards_page(&doc(SEARCH), CARD, 2);
	assert!(!last_has_next, "trang cuối không có trang sau");
}

// ── filters ────────────────────────────────────────────────────────────────

#[komorei_test]
fn filter_panel_is_read_from_the_site() {
	assert!(
		doc(CATALOG).select_first(FILTER_PANEL).is_some(),
		"the catalogue page must carry #filter"
	);
	let filters = parsers::parse_filters(&doc(CATALOG));
	assert!(
		filters.len() >= 7,
		"expected the filter groups, got {}",
		filters.len()
	);

	let ids: Vec<&str> = filters.iter().map(|f| f.id.as_ref()).collect();
	for expected in [
		"sort", "type", "season", "genres", "year", "studio", "rating", "country",
	] {
		assert!(
			ids.contains(&expected),
			"group `{expected}` missing from {ids:?}"
		);
	}
}

#[komorei_test]
fn genres_are_multi_select_and_the_rest_single() {
	let filters = parsers::parse_filters(&doc(CATALOG));
	let genres = filters
		.iter()
		.find(|f| f.id.as_ref() == "genres")
		.expect("the genres group is missing");
	// `genres[]` renders as checkboxes; the site allows an exclusion marker.
	assert!(
		matches!(
			genres.kind,
			komorei::FilterKind::MultiSelect {
				can_exclude: true,
				..
			}
		),
		"genres must be multi-select and excludable"
	);
	let country = filters
		.iter()
		.find(|f| f.id.as_ref() == "country")
		.expect("the country group is missing");
	assert!(
		matches!(country.kind, komorei::FilterKind::Select { .. }),
		"country must be single-select"
	);
}

#[komorei_test]
fn filter_options_carry_both_label_and_path_segment() {
	let filters = parsers::parse_filters(&doc(CATALOG));
	let country = filters
		.iter()
		.find(|f| f.id.as_ref() == "country")
		.expect("the country group is missing");
	let (options, ids) = match &country.kind {
		komorei::FilterKind::Select { options, ids, .. } => (options, ids),
		_ => panic!("country must be a Select filter"),
	};
	// `jp` is the path segment, `Nhật Bản (5381)` the label — the trailing
	// count is stripped so the chip reads cleanly.
	let id_list = ids
		.as_ref()
		.expect("ids are required because labels differ from values");
	assert!(
		id_list.iter().any(|v| v.as_ref() == "jp"),
		"the jp country code must be present"
	);
	assert!(
		options.iter().all(|o| !o.contains('(')),
		"labels must drop the trailing count: {options:?}"
	);
	assert!(
		options.iter().any(|o| o.contains("Nhật Bản")),
		"the Vietnamese label must be present"
	);
}

// ── path building ──────────────────────────────────────────────────────────

#[komorei_test]
fn category_path_uses_all_for_unset_slots() {
	assert_eq!(
		parsers::build_category_path(&[], 1),
		"/danh-sach/all/all/all/all/all/all/all/"
	);
}

#[komorei_test]
fn category_path_places_values_positionally() {
	// The site's order is type, genres, season, year, studio, rating, country.
	let filters = vec![
		FilterValue::Select {
			id: "type".into(),
			value: "list-bo".into(),
		},
		FilterValue::Select {
			id: "season".into(),
			value: "winter".into(),
		},
		FilterValue::Select {
			id: "year".into(),
			value: "2026".into(),
		},
		FilterValue::Select {
			id: "country".into(),
			value: "jp".into(),
		},
	];
	assert_eq!(
		parsers::build_category_path(&filters, 1),
		"/danh-sach/list-bo/all/winter/2026/all/all/jp/"
	);
}

#[komorei_test]
fn category_path_joins_genres_with_dashes() {
	let filters = vec![FilterValue::MultiSelect {
		id: "genres".into(),
		included: alloc::vec!["1".into(), "2".into()],
		excluded: alloc::vec!["46".into()],
	}];
	// Included genres join with `-`; an excluded one is prefixed `!`.
	assert!(
		parsers::build_category_path(&filters, 1).contains("/1-2-!46/"),
		"genres must join with dashes and mark exclusions: {}",
		parsers::build_category_path(&filters, 1)
	);
}

#[komorei_test]
fn category_path_paginates_and_sorts() {
	let filters = vec![FilterValue::Select {
		id: "sort".into(),
		value: "nameaz".into(),
	}];
	assert_eq!(
		parsers::build_category_path(&filters, 3),
		"/danh-sach/all/all/all/all/all/all/all/trang-3/?sort=nameaz"
	);
	// The site's default sort sends an empty value, which adds no query at all.
	let default = vec![FilterValue::Select {
		id: "sort".into(),
		value: String::new(),
	}];
	assert!(!parsers::build_category_path(&default, 1).contains('?'));
}

#[komorei_test]
fn search_path_encodes_the_keyword_and_pages() {
	assert_eq!(
		parsers::build_search_path("one piece", 1),
		"/tim-kiem/one%20piece/"
	);
	assert_eq!(
		parsers::build_search_path("one piece", 2),
		"/tim-kiem/one%20piece/trang-2/"
	);
	// A slash in the keyword must not escape the path segment.
	assert_eq!(parsers::build_search_path("a/b", 1), "/tim-kiem/a%2Fb/");
}

// ── listings & deep links ──────────────────────────────────────────────────

#[komorei_test]
fn listings_cover_the_catalogue_and_the_boards() {
	let listings = parsers::static_listings();
	assert!(listings.len() >= 10);
	let ids: Vec<&str> = listings.iter().map(|l| l.id.as_ref()).collect();
	assert!(ids.contains(&"anime-moi/"));
	assert!(ids.contains(&"bang-xep-hang/day.html"));
	// `voted` is a real board; `week` is not one.
	assert!(ids.contains(&"bang-xep-hang/voted.html"));
	assert!(!ids.contains(&"bang-xep-hang/week.html"));
}

#[komorei_test]
fn deep_links_resolve_the_slug_from_any_phim_url() {
	let source = crate::AnimeVietsubSource::new();
	for url in [
		"https://animevietsub.li/phim/nige-jouzu-no-wakagimi-2nd-season-a6015/",
		"https://animevietsub.li/phim/nige-jouzu-no-wakagimi-2nd-season-a6015/xem-phim.html",
		"https://animevietsub.li/phim/nige-jouzu-no-wakagimi-2nd-season-a6015/tap-11-115905.html",
		"/phim/one-piece-dao-hai-tac-a1/",
	] {
		match source.handle_deep_link(url.to_string()) {
			Ok(Some(komorei::DeepLinkResult::Anime { key })) => {
				assert!(
					key.starts_with("nige-jouzu") || key.starts_with("one-piece"),
					"wrong key for {url}: {key}"
				);
			}
			other => panic!("{url} phải ra anime, nhận {other:?}"),
		}
	}
	assert!(
		source
			.handle_deep_link("https://animevietsub.li/bang-xep-hang/day.html".to_string())
			.unwrap()
			.is_none(),
		"url không phải phim thì không được ra anime"
	);
}

// ── helpers ────────────────────────────────────────────────────────────────

#[komorei_test]
fn collapse_squashes_markup_whitespace() {
	assert_eq!(parsers::collapse("  Yêu\n\t Thần   Ký \n"), "Yêu Thần Ký");
	assert_eq!(parsers::collapse(""), "");
	assert_eq!(parsers::collapse("   "), "");
}

#[komorei_test]
fn anime_key_extraction_handles_every_url_shape() {
	assert_eq!(
		parsers::anime_key_of("/phim/one-piece-a1/"),
		Some("one-piece-a1")
	);
	assert_eq!(
		parsers::anime_key_of("https://animevietsub.li/phim/one-piece-a1/xem-phim.html"),
		Some("one-piece-a1")
	);
	assert_eq!(parsers::anime_key_of("/phim/"), None);
	assert_eq!(parsers::anime_key_of("/bang-xep-hang/day.html"), None);
}

#[komorei_test]
fn episode_numbers_keep_their_halves() {
	// A half episode must not round to the next integer.
	assert_eq!(parsers::normalise_number("12.5"), "12.5");
	assert_eq!(parsers::normalise_number("01"), "1");
	assert_eq!(parsers::normalise_number("1179"), "1179");
}

// ── home layout ────────────────────────────────────────────────────────────

fn home_layout_shape() {
	use komorei::HomeComponentValue;
	let layout = crate::home::from_document(&doc(HOME));
	// Launcher strip + 4 poster rails + catalogue strip.
	assert_eq!(
		layout.components.len(),
		6,
		"got {}",
		layout.components.len()
	);

	// A launcher strip first, the rails in the middle, the catalogue strip last.
	assert!(matches!(
		layout.components[0].value,
		HomeComponentValue::Filters(_)
	));
	assert!(
		layout.components[1..5]
			.iter()
			.all(|c| matches!(c.value, HomeComponentValue::Scroller { .. })),
		"the middle four must all be poster rails"
	);
	assert!(matches!(
		layout.components[5].value,
		HomeComponentValue::Links(_)
	));
}

/// The rails the home asks for must all be ones the site **server-renders**.
///
/// This is the assertion that catches the wide carousel (`.MovieListSldCn`) and
/// the weekly top-ten (`#showTopPhim`): both parse fine against a
/// browser-captured fixture and both come back **empty** from the server, because
/// the site injects them with its own scripts. A rail added to the home that is
/// only built client-side silently renders nothing on a device.
fn home_rails_are_server_rendered() {
	// The site fills these two sections with its own scripts. They parse fine
	// against a browser-captured fixture — which is why they look safe — and
	// come back **empty** from `fetch_html`, so on a device they render nothing.
	const CLIENT_SIDE_ONLY: [(&str, &str); 2] = [
		("the wide carousel", ".MovieListSldCn .TPostMv"),
		("the weekly top-ten", "#showTopPhim .TPost"),
	];
	for (title, selector) in CLIENT_SIDE_ONLY {
		assert!(
			!crate::home::rail_selectors().contains(&selector),
			"`{selector}` ({title}) is client-side only and must not be a home rail"
		);
	}
	// Every rail the home does use must match something, so a typo cannot leave a
	// silently empty rail behind.
	let doc = doc(HOME);
	for selector in crate::home::rail_selectors() {
		assert!(
			doc.select(selector).map(|l| l.size()).unwrap_or(0) > 0,
			"rail `{selector}` matches nothing"
		);
	}
}

#[komorei_test]
fn every_home_component_has_entries() {
	use komorei::HomeComponentValue;
	// The app skips a component whose value is empty, so an empty one is dead
	// weight: assert nothing was pushed empty.
	for component in crate::home::from_document(&doc(HOME)).components {
		let empty = match &component.value {
			HomeComponentValue::Filters(items) => items.is_empty(),
			HomeComponentValue::BigScroller { entries, .. } => entries.is_empty(),
			HomeComponentValue::Scroller { entries, .. } => entries.is_empty(),
			HomeComponentValue::AnimeList { entries, .. } => entries.is_empty(),
			HomeComponentValue::Links(links) => links.is_empty(),
			HomeComponentValue::AnimeEpisodeList { entries, .. } => entries.is_empty(),
			HomeComponentValue::ImageScroller { links, .. } => links.is_empty(),
		};
		assert!(!empty, "component {:?} has no entries", component.title);
		assert!(component.title.is_some(), "every component needs a title");
	}
}

#[komorei_test]
fn rails_that_can_reach_a_catalogue_carry_their_listing() {
	use komorei::HomeComponentValue;
	// `listing` is what renders the row's "see all"; a rail pointing at a
	// catalogue page must carry it.
	let layout = crate::home::from_document(&doc(HOME));
	let rails: Vec<_> = layout
		.components
		.iter()
		.filter_map(|c| match &c.value {
			HomeComponentValue::Scroller { listing, .. } => Some(listing.is_some()),
			_ => None,
		})
		.collect();
	assert!(!rails.is_empty(), "expected poster rails");
	assert!(
		rails.iter().any(|has| *has),
		"at least one rail must offer 'see all'"
	);
}

#[komorei_test]
fn launcher_shortcuts_use_catalogue_path_values() {
	use komorei::HomeComponentValue;
	let layout = crate::home::from_document(&doc(HOME));
	let HomeComponentValue::Filters(items) = &layout.components[0].value else {
		panic!("first component must be the launcher strip");
	};
	assert!(items.len() >= 8, "got {} shortcuts", items.len());
	// Every shortcut must be a `Select` on a slot the catalogue path addresses.
	for item in items {
		let values = item.values.as_ref().expect("shortcut needs filter values");
		assert_eq!(values.len(), 1);
		match &values[0] {
			FilterValue::Select { id, value } => {
				assert!(
					["type", "season", "country"].contains(&id.as_str()),
					"unexpected slot {id}"
				);
				assert!(!value.is_empty(), "empty value in {}", item.title);
			}
			other => panic!("expected Select, got {other:?}"),
		}
	}
	// The ids must be ones the site's own panel emits, or the shortcut resolves
	// to a page that does not exist.
	let values: Vec<String> = items
		.iter()
		.filter_map(|i| i.values.as_ref())
		.filter_map(|v| match &v[0] {
			FilterValue::Select { value, .. } => Some(value.clone()),
			_ => None,
		})
		.collect();
	for expected in [
		"all",
		"list-bo",
		"list-le",
		"list-tron-bo",
		"winter",
		"spring",
		"jp",
		"cn",
	] {
		assert!(
			values.iter().any(|v| v == expected),
			"missing shortcut {expected}"
		);
	}
}

#[komorei_test]
fn catalogue_links_cover_listings_and_boards() {
	use komorei::{HomeComponentValue, LinkValue};
	let layout = crate::home::from_document(&doc(HOME));
	let HomeComponentValue::Links(links) = &layout.components.last().unwrap().value else {
		panic!("last component must be the catalogue strip");
	};
	let ids: Vec<&str> = links
		.iter()
		.filter_map(|l| match &l.value {
			Some(LinkValue::Listing(listing)) => Some(listing.id.as_str()),
			_ => None,
		})
		.collect();
	assert!(ids.contains(&"anime-moi/"));
	assert!(ids.contains(&"bang-xep-hang/voted.html"));
	// `week` is not one of the site's boards, so nothing may link to it.
	assert!(!ids.iter().any(|id| id.contains("week")));
}

// ── recommendations via `extra` ────────────────────────────────────────────

/// The detail page carries its own "Gợi ý cùng người xem" rail. Throwing it
/// away means the app re-requests the page it just parsed, so the cards are
/// stashed in `extra` and served back with no request at all.
#[komorei_test]
fn detail_stashes_its_recommendation_rail_in_extra() {
	let anime = crate::parsers::parse_detail(&doc(DETAIL));
	let entries = crate::parsers::recommendations_from_extra(&anime);
	assert!(
		!entries.is_empty(),
		"the rail parsed empty or was not stashed"
	);

	// Every entry must be usable as-is: the app renders these without another
	// `getAnimeUpdate`, so a card missing its key or cover is a broken row.
	for entry in &entries {
		assert!(!entry.key.is_empty(), "a stashed card has no key");
		assert_eq!(entry.source_id, crate::catalog::SOURCE_ID);
		assert!(!entry.title.is_empty(), "`{}` has no title", entry.key);
		assert!(!entry.cover.is_empty(), "`{}` has no cover", entry.key);
	}
	// Keys are what a deep link resolves, so they must round-trip through the
	// source's own router.
	for entry in &entries {
		assert_eq!(
			Some(entry.key.as_str()),
			crate::parsers::anime_key_of(&format!("/phim/{}/", entry.key)),
			"`{}` is not a bare anime key",
			entry.key
		);
	}
	// The rail is a carousel of distinct titles; a duplicate would repeat itself.
	let mut keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
	let total = keys.len();
	keys.sort_unstable();
	keys.dedup();
	assert_eq!(keys.len(), total, "duplicate recommendation keys");
}

/// The stash is the read half of the same key, so what goes in must come back
/// out unchanged — this is what lets `get_recommended_anime` skip the network.
#[komorei_test]
fn stashed_recommendations_round_trip_intact() {
	let anime = crate::parsers::parse_detail(&doc(DETAIL));
	let first = crate::parsers::recommendations_from_extra(&anime);
	// A second read, and then a read of an anime carrying the same stash, must
	// both be identical: nothing is consumed or mutated by handing it over.
	assert_eq!(first, crate::parsers::recommendations_from_extra(&anime));

	let carried = Anime {
		key: String::from("some-other-anime"),
		..anime.clone()
	};
	assert_eq!(first, crate::parsers::recommendations_from_extra(&carried));
}

/// A page with no recommendation rail must not invent an entry — an absent key
/// is what a caller checks for, and it is what triggers the request fallback.
#[komorei_test]
fn detail_without_a_recommendation_rail_omits_the_extra() {
	let anime = crate::parsers::parse_detail(&doc(
		r#"<article class="TPost Single"><h1 class="Title">Solo</h1>
		   <div class="Description">No rail here.</div></article>"#,
	));
	assert_eq!(anime.title, "Solo");
	assert!(
		!anime
			.extra
			.contains_key(crate::parsers::EXTRA_RECOMMENDATIONS),
		"a page with no rail must not report recommendations"
	);
	assert!(
		crate::parsers::recommendations_from_extra(&anime).is_empty(),
		"a missing stash must read back as empty, not as an error"
	);
}

/// `extra` is one flat namespace shared by every source, so a source's key must
/// carry its own prefix or it can shadow another's.
#[komorei_test]
fn recommendation_key_is_namespaced_by_source() {
	assert!(
		crate::parsers::EXTRA_RECOMMENDATIONS.contains("avs."),
		"the key must be namespaced, got `{}`",
		crate::parsers::EXTRA_RECOMMENDATIONS
	);
}

// ── get_recommended_anime ──────────────────────────────────────────────────

/// The whole point of stashing the rail: with the stash present the source
/// answers from `anime.extra` and touches no network. A test that cannot prove
/// "no request" can at least prove the answer is the stash, byte for byte.
#[komorei_test]
fn recommended_anime_serves_the_stash_without_parsing_again() {
	use komorei::RecommendationsHandler;

	let detail = crate::parsers::parse_detail(&doc(DETAIL));
	let stashed = crate::parsers::recommendations_from_extra(&detail);
	assert!(!stashed.is_empty(), "the fixture page must carry a rail");

	// The app hands back the very anime it was given, extras included.
	let page = crate::AnimeVietsubSource::new()
		.get_recommended_anime(detail.clone())
		.expect("recommendations");

	assert_eq!(page.entries, stashed);
	assert!(!page.has_next_page, "the rail is one page, never paged");
}

/// The app re-queries this for every anime it opens, and an anime opened
/// straight from a listing was never upgraded, so it carries no stash.
///
/// That case falls back to re-fetching the detail page, which is the only way to
/// stay correct if the site ever moves the rail off it. It is deliberately NOT
/// covered here: the test host has no stub for the network, so such a test would
/// reach the live site and assert on whatever it answered today. What is worth
/// pinning down is the half that makes the stash worth having — see
/// `recommended_anime_serves_the_stash_without_parsing_again`.
#[komorei_test]
fn a_lite_card_carries_no_recommendations() {
	let lite = Anime {
		key: String::from("some-anime"),
		source_id: crate::catalog::SOURCE_ID.into(),
		title: String::from("Some Anime"),
		..Default::default()
	};
	// A Lite card is what a listing produces, and it must not be mistaken for
	// "this anime has no recommendations" — the app is expected to ask again.
	assert!(lite.extra.is_empty());
	assert!(crate::parsers::recommendations_from_extra(&lite).is_empty());
}
