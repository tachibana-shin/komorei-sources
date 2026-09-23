//! HTML parsing: listing cards, detail metadata, episode lists, live-search
//! dropdowns, the obfuscated grab playlist, and the filter-based list url
//! builder. All selectors were verified against the live site markup.

use alloc::{
	format,
	string::{String, ToString},
	vec,
	vec::Vec,
};
use komorei::{
	Anime, AnimeSeason, AnimeStatus, CategoryLink, Episode, FilterValue,
	imports::base64,
	imports::html::{Document, Element},
};

use crate::catalog::{COUNTRIES, GENRES, SERVERS, SOURCE_ID, TYPE_FILTERS};
use crate::models::{DetailInfo, PlaylistEntry};
use crate::util::{
	absolutize, link_base, page_param, parse_episode_number, parse_first_f32, parse_first_int,
	parse_two_ints,
};

/// The Last page number reachable via `ul#yw0.Pager`; has next if any page
/// link exceeds the current page.
pub(crate) fn has_next_page(doc: &Document, page: i32) -> bool {
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

/// Scan raw HTML for `"…"`-quoted base64 tokens and decode the first one that
/// parses as a playlist array (a `[{file,…}]` JSON). This mirrors the player's
/// `JSON.parse(atob(v<hex>))`; the decode itself is the runner's native
/// `imports::base64`.
pub(crate) fn extract_playlist(html: &str) -> Option<Vec<PlaylistEntry>> {
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
			&& let Some(raw) = base64::decode(&token)
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

/// Parse a listing/grid card (`.item-file-index`) into a Lite anime.
pub(crate) fn from_card(card: &Element, base: &str) -> Option<Anime> {
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

/// Parse the metadata rows of a detail page into a [DetailInfo] bag.
pub(crate) fn parse_detail(doc: &Document) -> DetailInfo {
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
pub(crate) fn parse_episodes(doc: &Document, base: &str, _film_key: &str) -> Vec<Episode> {
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
pub(crate) fn parse_list_page(doc: &Document, base: &str, page: i32) -> (Vec<Anime>, bool) {
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

/// Parse the live-search dropdown (`li.result-item`). The site nests `<a>`
/// links (country/year) INSIDE the film `<a>` — invalid HTML that jsoup
/// resolves by SPLITTING the outer anchor into sibling fragments: the first
/// fragment keeps the href/title on every copy, but only one of them carries
/// the cover `<img>`. Reading the cover from the *anchor* therefore loses it
/// (the dedup keeps the first, img-less fragment). All fields are read per
/// `li` instead: link/title from the first film anchor (its attrs are cloned
/// onto every fragment), cover + original title from the li.
pub(crate) fn parse_search_items(doc: &Document, base: &str) -> Vec<Anime> {
	let mut out: Vec<Anime> = Vec::new();
	let Some(items) = doc.select("li.result-item") else {
		return out;
	};
	for i in 0..items.size() {
		let Some(li) = items.get(i) else { continue };
		let Some(a) = li.select_first("a[href*='-f'][href$='.html']") else {
			continue;
		};
		let Some(href) = a.attr("href") else { continue };
		let path = href.trim_start_matches('/');
		if !path.contains("-f") || !path.ends_with(".html") {
			continue;
		}
		let title = a.attr("title").unwrap_or_default().trim().to_string();
		if title.is_empty() {
			continue;
		}
		let original = li
			.select_first(".result-item-title-en")
			.and_then(|e| e.text())
			.map(|t| t.trim().to_string())
			.unwrap_or_default();
		let cover = li
			.select_first(".result-item-image img")
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

/// Episode badge for the "Mới Cập Nhật" home rail.
pub(crate) fn home_episode(card: &Anime) -> Episode {
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

/// Full detail: metadata + seasons (PAI/NGC).
pub(crate) fn build_full(base: &str, key: &str, info: DetailInfo) -> Anime {
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

/// Build the site listing url from search filters (first selected wins:
/// type → genre → country → year, then "Mới Cập Nhật").
pub(crate) fn build_list_url(base: &str, page: i32, filters: &[FilterValue]) -> String {
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

pub(crate) fn select_value<'a>(filters: &'a [FilterValue], id: &str) -> Option<&'a str> {
	filters.iter().find_map(|f| match f {
		FilterValue::Select { id: f_id, value } if f_id == id => Some(value.as_str()),
		_ => None,
	})
}

pub(crate) fn multi_included<'a>(filters: &'a [FilterValue], id: &str) -> Option<&'a str> {
	filters.iter().find_map(|f| match f {
		FilterValue::MultiSelect {
			id: f_id,
			included,
			..
		} if f_id == id => included.first().map(String::as_str),
		_ => None,
	})
}