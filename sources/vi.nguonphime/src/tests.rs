//! Crate tests: pure parsing/fixture coverage run on the wasm test host. The
//! base64 decode exercised here goes through the runner's native `base64`
//! import (implemented by wasmer in the test host, wasmi in the app).

use alloc::{
	string::ToString,
	vec,
	vec::Vec,
};
use crate::catalog::*;
use crate::parsers::*;
use crate::util::*;
use komorei::{
	AnimeStatus, FilterValue,
	imports::html::Html,
	imports::std::current_date,
};
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
fn base64_import_roundtrips_standard_and_padded() {
	use komorei::imports::base64;
	// The runner-native decode tolerates plain and padded standard base64.
	assert_eq!(base64::decode("TWFu").as_deref(), Some(&b"Man"[..]));
	assert_eq!(base64::decode("TWE=").as_deref(), Some(&b"Ma"[..]));
	assert_eq!(base64::decode("TQ==").as_deref(), Some(&b"M"[..]));
	assert_eq!(base64::decode("aGVsbG8=").as_deref(), Some(&b"hello"[..]));
	// invalid chars / bad length → None
	assert!(base64::decode("he!!o").is_none());
	assert!(base64::decode("TQ=").is_none());
	// encode round-trips standard-with-padding
	assert_eq!(base64::encode(b"KomORei"), "S29tT1JlaQ==");
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