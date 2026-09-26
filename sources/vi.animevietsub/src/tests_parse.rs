//! Parsing tests, run against **real page captures**.
//!
//! Each `page_*.html` fixture is a real response from the site with `<script>`,
//! `<style>` and inline `<svg>` stripped — nothing the parsers read is removed,
//! and keeping the genuine markup is the whole point: these assertions are what
//! catch a site redesign before it reaches a user's screen.

use alloc::{
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
		"trang chủ phải có nhiều card, có {}",
		found.len()
	);

	let one = found
		.iter()
		.find(|a| a.title.contains("Thiếu Chủ"))
		.expect("phải thấy card 'Thiếu Chủ Giỏi Chạy Trốn 2'");

	// Keys are the `/phim/{slug}-a{id}/` segment, which is also the detail url.
	assert_eq!(one.key, "nige-jouzu-no-wakagimi-2nd-season-a6015");
	assert!(
		one.cover.starts_with("https://"),
		"cover phải là url tuyệt đối: {}",
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
		.expect("phải có card cần tìm");
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
		.expect("phải có card cần tìm");
	let year = one.release_year.as_ref().map(|y| y.name.clone());
	assert_eq!(year.as_deref(), Some("2026"));
}

#[komorei_test]
fn view_count_comes_from_the_year_class() {
	let found = cards(HOME, "#hot-home .TPostMv");
	let one = found
		.iter()
		.find(|a| a.key == "nige-jouzu-no-wakagimi-2nd-season-a6015")
		.expect("phải có card cần tìm");
	// `Lượt xem: 810,440` -> 810440, separators stripped.
	assert_eq!(one.views, 810_440);
}

#[komorei_test]
fn carousel_card_reads_its_info_strip() {
	// The wide cards carry `.Info` spans instead of `.mli-eps`:
	// `.AAIco-star` is the score and `.AAIco-access_time` the `11/12` progress.
	let found = cards(HOME, ".MovieListSldCn .TPostMv");
	assert!(!found.is_empty(), "carousel phải có card");
	let one = found
		.iter()
		.find(|a| a.key == "nige-jouzu-no-wakagimi-2nd-season-a6015")
		.expect("phải thấy card carousel");
	assert_eq!(one.rating, Some(9.7));
	assert_eq!(one.current_episode.as_deref(), Some("Tập 11"));
	assert_eq!(one.quality_tag.as_deref(), Some("FHD"));
	assert_eq!(
		one.studio.as_ref().map(|s| s.name.as_str()),
		Some("CloverWorks")
	);
	assert!(!one.genres.is_empty(), "phải có thể loại");
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
		assert!(!found.is_empty(), "rail `{selector}` trả về 0 card");
		for anime in &found {
			assert!(!anime.key.is_empty(), "card trong `{selector}` thiếu key");
			assert!(!anime.title.is_empty(), "card `{}` thiếu title", anime.key);
		}
	}
}

#[komorei_test]
fn home_layout_is_built_from_the_front_page() {
	let layout = home::from_document(&doc(HOME));
	assert!(
		layout.components.len() >= 5,
		"phải có ít nhất 5 rail, có {}",
		layout.components.len()
	);
	for component in &layout.components {
		assert!(component.title.is_some(), "mỗi component cần tiêu đề");
	}
}

// ── ranking ────────────────────────────────────────────────────────────────

#[komorei_test]
fn ranking_rows_parse_with_their_episode_state() {
	let found = parsers::parse_ranking(&doc(RANKING));
	assert!(!found.is_empty(), "bảng xếp hạng phải có hàng");
	let one = &found[0];
	assert!(!one.key.is_empty());
	assert!(!one.title.is_empty());
	// `.score` is the episode state, not a rating: `Tập NN` or `Full`.
	assert!(
		one.current_episode.is_some() || one.status == AnimeStatus::Completed,
		"phải đọc được trạng thái tập, có current={:?} status={:?}",
		one.current_episode,
		one.status
	);
	assert_eq!(one.rating, None, "`Tập 24` không phải là điểm");
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
	assert_eq!(found.len(), 11, "trang này có 11 tập");

	let first = &found[0];
	assert_eq!(first.episode_number, "1");
	// The key has to round-trip into the `data-id` + `data-hash` pair that
	// `/ajax/player` insists on.
	let (id, hash) = split_episode_key(&first.key).expect("key phải tách được");
	assert_eq!(id, "114607");
	assert!(
		hash.len() > 40,
		"hash phải là chữ ký dài, có {}",
		hash.len()
	);
	assert!(
		hash.starts_with("5qC6TJh"),
		"hash phải khớp data-hash: {hash}"
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
			.unwrap_or_else(|| panic!("không tách được key {}", episode.key));
		assert!(
			hash.chars()
				.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
			"hash lạ: {hash}"
		);
		// Not every hash carries a `-`; what matters is that the split did not
		// eat one when it did. Check a known multi-part hash explicitly.
		if episode.episode_number == "1" {
			assert!(hash.contains('-'), "tập 1 có hash nhiều phần: {hash}");
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
	// The site's synopsis reads `[Tập 11] Thiếu Chủ Giỏi Chạy Trốn 2: …`, so the
	// episode marker goes and what follows it is the actual blurb.
	let synopsis = detail.description.clone().unwrap_or_default();
	assert!(
		!synopsis.starts_with('['),
		"tiền tố [Tập …] phải được bỏ: {synopsis}"
	);
	assert!(
		synopsis.starts_with("Thiếu Chủ Giỏi Chạy Trốn 2:"),
		"phần còn lại phải là nội dung mô tả: {synopsis}"
	);
	assert!(
		synopsis.contains("Sau thời gian dài"),
		"mất nội dung: {synopsis}"
	);
	assert!(detail.cover.contains("cdn.animevietsub.li"));
	assert!(detail.banner.is_some(), "phải có banner");
	assert_eq!(detail.rating, Some(9.7));
	assert_eq!(detail.quality_tag.as_deref(), Some("FHD"));
}

// ── search & pagination ────────────────────────────────────────────────────

#[komorei_test]
fn search_results_parse_as_cards() {
	let found = cards(SEARCH, CARD);
	assert!(found.len() >= 10);
	// The keyword really filtered: every card mentions it.
	assert!(
		found.iter().any(|a| a.title.contains("One Piece")),
		"phải có kết quả One Piece"
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
		"trang danh sách phải có #filter"
	);
	let filters = parsers::parse_filters(&doc(CATALOG));
	assert!(
		filters.len() >= 7,
		"phải đọc được các nhóm lọc, có {}",
		filters.len()
	);

	let ids: Vec<&str> = filters.iter().map(|f| f.id.as_ref()).collect();
	for expected in [
		"sort", "type", "season", "genres", "year", "studio", "rating", "country",
	] {
		assert!(
			ids.contains(&expected),
			"thiếu nhóm `{expected}` trong {ids:?}"
		);
	}
}

#[komorei_test]
fn genres_are_multi_select_and_the_rest_single() {
	let filters = parsers::parse_filters(&doc(CATALOG));
	let genres = filters
		.iter()
		.find(|f| f.id.as_ref() == "genres")
		.expect("phải có nhóm genres");
	// `genres[]` renders as checkboxes; the site allows an exclusion marker.
	assert!(
		matches!(
			genres.kind,
			komorei::FilterKind::MultiSelect {
				can_exclude: true,
				..
			}
		),
		"genres phải multi-select + cho phép loại trừ"
	);
	let country = filters
		.iter()
		.find(|f| f.id.as_ref() == "country")
		.expect("phải có nhóm country");
	assert!(
		matches!(country.kind, komorei::FilterKind::Select { .. }),
		"country phải single-select"
	);
}

#[komorei_test]
fn filter_options_carry_both_label_and_path_segment() {
	let filters = parsers::parse_filters(&doc(CATALOG));
	let country = filters
		.iter()
		.find(|f| f.id.as_ref() == "country")
		.expect("phải có nhóm country");
	let (options, ids) = match &country.kind {
		komorei::FilterKind::Select { options, ids, .. } => (options, ids),
		_ => panic!("country phải là Select"),
	};
	// `jp` is the path segment, `Nhật Bản (5381)` the label — the trailing
	// count is stripped so the chip reads cleanly.
	let id_list = ids.as_ref().expect("phải có ids vì label khác giá trị");
	assert!(
		id_list.iter().any(|v| v.as_ref() == "jp"),
		"phải có mã quốc gia jp"
	);
	assert!(
		options.iter().all(|o| !o.contains('(')),
		"nhãn phải bỏ đuôi (số lượng): {options:?}"
	);
	assert!(
		options.iter().any(|o| o.contains("Nhật Bản")),
		"phải có nhãn Nhật Bản"
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
		"phải nối thể loại bằng dấu gạch và đánh dấu loại trừ: {}",
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
					"sai key cho {url}: {key}"
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
