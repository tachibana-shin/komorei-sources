//! The jsoup parsing layer.
//!
//! Every selector here was read off the live site rather than guessed. The site
//! is server-rendered, so all of this is markup scraping. One wrinkle worth
//! knowing: posters are served from a separate CDN host, and a lazy-loading
//! `src` may be a placeholder — hence [`image_url`] also looks at `data-src`
//! and Cloudflare's `data-cfsrc`.

extern crate alloc;

use alloc::{
	borrow::Cow,
	format,
	string::{String, ToString},
	vec,
	vec::Vec,
};

use komorei::{
	Anime, AnimeSeason, AnimeStatus, CategoryLink, Episode, Filter, FilterValue, HashMap, Listing,
	ListingKind, MultiSelectFilter, SelectFilter,
	helpers::uri::encode_uri_component,
	imports::html::{Document, Element},
};

use crate::catalog::{
	CATEGORY_SLOTS, FILTER_GENRES, FILTER_TYPE, LISTINGS, RANKING_TYPES, SLOT_ANY, SOURCE_ID,
};

/// Cards are `.TPostMv` everywhere — the home rails, the category grids, the
/// search results and the static listings all use the same block.
pub const CARD: &str = ".TPostMv";
/// The full ordered episode list lives on `/phim/{id}/xem-phim.html`; every
/// episode anchor carries its own `data-id` and `data-hash`.
pub const EPISODES: &str = "#list-server .list-episode .episode a";
/// The episode widget itself, and its individual items — used to tell "the page
/// has no episode list" apart from "the page is not what we asked for".
pub const LIST_SERVER: &str = "#list-server";
pub const EPISODE_ITEMS: &str = "#list-server li.episode";
/// The filter panel, whose input names double as the `/danh-sach/` slots.
pub const FILTER_PANEL: &str = "#filter";
/// The ranking boards render `li.group` rows, not `.TPostMv` cards.
pub const RANKING_ROW: &str = "li.group";

/// Whether a page is Cloudflare's interstitial rather than the content asked
/// for.
///
/// The managed challenge answers 200 with a near-empty body whose `<title>` is
/// empty, so a status check alone is not enough. The app retries such a
/// response through a WebView; this only reports that the shape was seen.
pub fn looks_like_challenge(doc: &Document) -> bool {
	let title_is_empty = doc
		.select_first("title")
		.and_then(|e| e.text())
		.map(|t| collapse(&t).is_empty())
		.unwrap_or(false);
	let has_content = doc
		.select("h1, .TPost, .TPostMv, #list-server")
		.map(|l| l.size())
		.unwrap_or(0)
		> 0;
	title_is_empty && !has_content
}

/// One grid card, as a Lite [`Anime`].
///
/// The site renders two card shapes out of the same `.TPostMv` block. The
/// common one wraps a poster `<img>`, a `.mli-eps` badge (`TẬP` + an `<i>`) and
/// an `.anime-avg-user-rating` score. The wide carousel variant carries a
/// `.TPostBg` banner and an `.Info` strip that states the same facts under
/// other class names — `.AAIco-star` for the score, `.AAIco-access_time` for the
/// `11/12` progress. Both shapes are read here.
pub fn parse_card(element: &Element) -> Option<Anime> {
	let anchor = element.select_first("a[href]")?;
	let href = anchor.attr("href")?;
	let key = anime_key_of(&href)?.to_string();

	let cover = image_url(&anchor);
	let title = element
		.select_first(".Title")
		.and_then(|e| e.text())
		.map(|t| collapse(&t))
		.filter(|t| !t.is_empty())
		.or_else(|| anchor.attr("title").map(|t| collapse(&t)))
		.unwrap_or_else(|| key.clone());

	let badge = parse_episode_badge(element);
	let rating = element
		.select_first(".anime-avg-user-rating")
		.or_else(|| element.select_first(".AAIco-star"))
		.and_then(|e| e.text())
		.and_then(|t| parse_rating(&t));

	// The release year lives in `.AAIco-date_range`. `.Year` is a red herring
	// here: on this site it holds the *view count*, as `Lượt xem: 7,524,686`.
	let release_year = element
		.select_first(".AAIco-date_range")
		.and_then(|e| e.text())
		.and_then(|t| first_number(&t))
		.map(|year| CategoryLink {
			name: year.to_string(),
			filters: Vec::new(),
		});

	let views = element
		.select_first(".Year")
		.and_then(|e| e.text())
		.map(|t| parse_count(&t))
		.unwrap_or(0);

	let quality_tag = element
		.select_first(".Qlty")
		.or_else(|| element.select_first(".mli-quality"))
		.and_then(|e| e.text())
		.map(|t| collapse(&t))
		.filter(|t| !t.is_empty());

	// `<p class="Studio AAIco-videocam"><span>Studio:</span> CloverWorks <i/></p>`
	let studio = element
		.select_first(".Studio")
		.and_then(|e| e.text())
		.map(|t| studio_name(&collapse(&t)))
		.filter(|n| !n.is_empty())
		.map(|name| CategoryLink {
			name,
			filters: Vec::new(),
		});

	// `.Genre > a` carries genre *names* only — the numeric ids the filter path
	// needs live in the `#filter` panel — so these are display-only links.
	let genres: Vec<CategoryLink> = element
		.select(".Genre > a")
		.map(|list| {
			list.filter_map(|a| {
				let name = a.text().map(|t| collapse(&t)).filter(|t| !t.is_empty())?;
				Some(CategoryLink {
					name,
					filters: Vec::new(),
				})
			})
			.collect()
		})
		.unwrap_or_default();

	Some(Anime {
		key,
		source_id: SOURCE_ID.into(),
		title,
		cover,
		rating,
		views,
		episode_count: badge.total.unwrap_or(0),
		current_episode: badge.current,
		release_year,
		genres,
		studio,
		quality_tag,
		status: badge.status,
		..Default::default()
	})
}

/// The episode state of a card.
struct Badge {
	current: Option<String>,
	total: Option<i32>,
	status: AnimeStatus,
}

/// Read the episode badge off a card.
///
/// `HOÀNTẤT` (and the carousel's `Full`) is the site's only "fully released"
/// marker, and it appears in place of the `TẬP` badge — so it doubles as the
/// airing status.
fn parse_episode_badge(element: &Element) -> Badge {
	let none = Badge {
		current: None,
		total: None,
		status: AnimeStatus::Unknown,
	};
	// Poster card: `<span class="mli-eps">TẬP<i>11</i></span>`.
	if let Some(badge) = element.select_first(".mli-eps") {
		let text = badge.text().map(|t| collapse(&t)).unwrap_or_default();
		if is_finished(&text) {
			return Badge {
				current: None,
				total: None,
				status: AnimeStatus::Completed,
			};
		}
		// The `<i>` holds just the number; the wrapper text is the fallback.
		let number = badge
			.select_first("i")
			.and_then(|e| e.text())
			.map(|t| normalise_number(&t))
			.filter(|n| !n.is_empty())
			.or_else(|| {
				let n = normalise_number(&text);
				if n.is_empty() { None } else { Some(n) }
			});
		if let Some(number) = number {
			return Badge {
				current: Some(format!("Tập {number}")),
				total: parse_total(&text),
				status: AnimeStatus::Unknown,
			};
		}
	}

	// Carousel card: `<span class="Time AAIco-access_time">11/12</span>`, where
	// an ongoing title reads `24/??` and a finished one `Full`.
	if let Some(progress) = element.select_first(".AAIco-access_time") {
		let text = progress.text().map(|t| collapse(&t)).unwrap_or_default();
		if is_finished(&text) {
			return Badge {
				current: None,
				total: None,
				status: AnimeStatus::Completed,
			};
		}
		let (current, total) = split_progress(&text);
		if let Some(number) = current {
			return Badge {
				current: Some(format!(
					"Tập {}",
					normalise_number(&alloc::format!("{number}"))
				)),
				total,
				status: AnimeStatus::Unknown,
			};
		}
	}

	none
}

/// Whether a badge reads as "fully released" — `HOÀNTẤT` or `Full`.
fn is_finished(text: &str) -> bool {
	let compact: String = text
		.to_lowercase()
		.chars()
		.filter(|c| c.is_alphanumeric())
		.collect();
	compact.contains("hoantat") || compact.contains("full")
}

/// All cards on a page, in document order.
pub fn parse_cards(doc: &Document, selector: &str) -> Vec<Anime> {
	doc.select(selector)
		.map(|list| list.filter_map(|el| parse_card(&el)).collect())
		.unwrap_or_default()
}

/// One page of cards plus whether another page exists.
///
/// The site paginates with a `.wp-pagenavi` strip whose anchors carry the page
/// number in `data`; the last entry is repeated as "Trang Cuối", so the maximum
/// is read off the attributes rather than counted.
pub fn parse_cards_page(doc: &Document, selector: &str, page: i32) -> (Vec<Anime>, bool) {
	let cards = parse_cards(doc, selector);
	let last = doc
		.select(".wp-pagenavi a")
		.map(|list| {
			list.filter_map(|a| a.attr("data").and_then(|d| d.trim().parse::<i32>().ok()))
				.fold(1, |acc, n| if n > acc { n } else { acc })
		})
		.unwrap_or(1);
	(cards, last > page)
}

/// A ranking board, which lists one `li.group` row per title.
///
/// A row is `<li class="po-NN group">` holding a 45×60 `.thumb` poster, the
/// short title in `.title-item a` and — despite the name — `.score`, which
/// carries the *episode* state (`Tập 24`, `Full`), not a rating.
pub fn parse_ranking(doc: &Document) -> Vec<Anime> {
	doc.select(RANKING_ROW)
		.map(|list| {
			list.filter_map(|row| {
				let anchor = row
					.select_first(".title-item a")
					.or_else(|| row.select_first("a[href]"))?;
				let href = anchor.attr("href")?;
				let key = anime_key_of(&href)?.to_string();
				let title = row
					.select_first(".title-item")
					.and_then(|e| e.text())
					.map(|t| collapse(&t))
					.filter(|t| !t.is_empty())
					.or_else(|| anchor.attr("title").map(|t| collapse(&t)))
					.unwrap_or_else(|| key.clone());
				let cover = row
					.select_first(".thumb img")
					.map(|img| {
						img.attr("src")
							.filter(|s| !s.is_empty())
							.or_else(|| img.attr("data-src").filter(|s| !s.is_empty()))
							.unwrap_or_default()
					})
					.map(|src| absolute(&src))
					.unwrap_or_default();
				// `.score` is the episode state, not a score — `Full` means the
				// title is fully released.
				let state = row
					.select_first(".rank-score .score")
					.and_then(|e| e.text())
					.map(|t| collapse(&t))
					.filter(|t| !t.is_empty());
				let (current_episode, status) = match state.as_deref() {
					Some(text) if text.to_lowercase().contains("full") => {
						(None, AnimeStatus::Completed)
					}
					Some(text) => (Some(text.to_string()), AnimeStatus::Unknown),
					None => (None, AnimeStatus::Unknown),
				};
				Some(Anime {
					key,
					source_id: SOURCE_ID.into(),
					title,
					cover,
					current_episode,
					status,
					..Default::default()
				})
			})
			.collect()
		})
		.unwrap_or_default()
}

/// One title's metadata, read from its detail page.
///
/// The page is an `article.TPost.Single`: its `<header>` carries `h1.Title`,
/// `h2.SubTitle` and the `.Image` poster, and the two metadata columns
/// (`.mvici-left` / `.mvici-right`) are `.InfoList` blocks of `.AAIco-adjust`
/// rows whose leading `<strong>` is the label (`Đạo diễn:`, `Quốc gia:`,
/// `Studio:`, …). Fields are therefore found **by label**, not by position —
/// the rows come and go as the site adds metadata.
pub fn parse_detail(doc: &Document) -> Anime {
	let title = doc
		.select_first("h1.Title")
		.or_else(|| doc.select_first("h1"))
		.and_then(|e| e.text())
		.map(|t| collapse(&t))
		.filter(|t| !t.is_empty())
		.unwrap_or_default();

	// The romaji/Japanese title, often several comma-separated aliases.
	let original_title = doc
		.select_first("h2.SubTitle")
		.and_then(|e| e.text())
		.map(|t| collapse(&t))
		.filter(|t| !t.is_empty())
		.unwrap_or_default();

	let og_image = doc
		.select_first("meta[property=og:image]")
		.and_then(|e| e.attr("content"))
		.filter(|s| !s.is_empty());

	let cover = doc
		.select_first(".Image img")
		.and_then(|e| e.attr("src"))
		.filter(|s| !s.is_empty())
		.or_else(|| {
			doc.select_first("figure.Objf img")
				.and_then(|e| e.attr("src"))
		})
		.or_else(|| og_image.clone())
		.map(|s| absolute(&s))
		.unwrap_or_default();

	let banner = doc
		.select_first(".TPostBg img")
		.and_then(|e| e.attr("src"))
		.filter(|s| !s.is_empty())
		.or(og_image)
		.map(|s| absolute(&s));

	let description = doc
		.select_first(".Description")
		.and_then(|e| e.text())
		.map(|t| strip_episode_marker(&collapse(&t)))
		.filter(|t| !t.is_empty());

	// `#average_score` is the score itself; `.num-rating` is how many rated it.
	let rating = doc
		.select_first("#average_score")
		.or_else(|| doc.select_first(".anime-avg-user-rating"))
		.and_then(|e| e.text())
		.and_then(|t| parse_rating(&t));
	let rating_count = doc
		.select_first(".num-rating")
		.and_then(|e| e.text())
		.and_then(|t| first_number(&t));

	// `809,991 Lượt Xem`
	let views = doc
		.select_first(".AAIco-remove_red_eye")
		.and_then(|e| e.text())
		.map(|t| parse_count(&t))
		.unwrap_or(0);

	// `.AAIco-access_time` reads `11/12`; an ongoing title shows `24/??`.
	let (current, total) = doc
		.select_first(".AAIco-access_time")
		.and_then(|e| e.text())
		.map(|t| split_progress(&t))
		.unwrap_or((None, None));

	// `current_episode` is rendered by the app as `Cập nhật tới tập %1$s`, so it
	// carries the bare progress — `22/24` — not a `Tập 22` label. (Cards use the
	// `Tập N` form instead, because there the field *is* the whole label.)
	let progress_label = match (current, total) {
		(Some(number), Some(total)) => Some(format!(
			"{}/{}",
			normalise_number(&format!("{number}")),
			total
		)),
		(Some(number), None) => Some(normalise_number(&format!("{number}"))),
		(None, _) => None,
	};

	let quality_tag = doc
		.select_first(".Qlty")
		.and_then(|e| e.text())
		.map(|t| collapse(&t))
		.filter(|t| !t.is_empty());

	// The year link points at `/danh-sach/all/all/all/2026`, so its text *is* the
	// `year` slot value and tapping it re-runs the catalogue filtered by year.
	let release_year = doc
		.select_first(".AAIco-date_range a")
		.and_then(|a| a.text().or_else(|| a.attr("title")))
		.map(|t| collapse(&t))
		.and_then(|t| first_number(&t))
		.map(|year| CategoryLink {
			name: year.to_string(),
			filters: alloc::vec![FilterValue::Select {
				id: String::from("year"),
				value: year.to_string(),
			}],
		});

	// Genres live in the schema.org breadcrumb. Only the `/the-loai/` entries
	// are genres — the trail also names the section and the title itself.
	let mut genres: Vec<CategoryLink> = doc
		.select("ol[itemprop=breadcrumb] li a")
		.map(|list| {
			list.filter_map(|a| {
				let href = a.attr("href")?;
				if !path_of(&href).starts_with("/the-loai/") {
					return None;
				}
				let name = a.text().map(|t| collapse(&t)).filter(|t| !t.is_empty())?;
				Some(CategoryLink {
					name,
					filters: Vec::new(),
				})
			})
			.collect()
		})
		.unwrap_or_default();

	let left = InfoList::in_column(doc, "mvici-left");
	let right = InfoList::in_column(doc, "mvici-right");

	// Some pages carry no breadcrumb and list the genres in the left column
	// instead, so fall back to that rather than showing none.
	if genres.is_empty() {
		genres = left.plain_links("thể loại");
	}

	let authors = left.plain_links("đạo diễn");
	let countries = left.plain_links("quốc gia");

	// Only `studio` maps onto a slot the catalogue understands: the link is
	// `/studio/CloverWorks.html`, whose last segment is exactly the `studio`
	// slot's value. The other links lead to their own landing pages, so they
	// stay plain labels.
	let studio = right
		.filtered_links("studio", "studio")
		.into_iter()
		.next()
		.map(|(name, filters)| CategoryLink { name, filters });
	let season_of = right.plain_links("season").into_iter().next();

	// Franchise parts: `.season_item > a` links the sibling seasons.
	let seasons: Vec<AnimeSeason> = doc
		.select(".season_item > a")
		.map(|list| {
			list.filter_map(|a| {
				let href = a.attr("href")?;
				let anime_id = anime_key_of(&href)?.to_string();
				let title = a.text().map(|t| collapse(&t)).filter(|t| !t.is_empty())?;
				Some(AnimeSeason {
					id: anime_id.clone(),
					title,
					anime_id,
				})
			})
			.collect()
		})
		.unwrap_or_default();

	Anime {
		title,
		source_id: SOURCE_ID.into(),
		original_title,
		cover,
		banner,
		description,
		rating,
		rating_count,
		views,
		episode_count: total.unwrap_or(0),
		current_episode: progress_label,
		release_year,
		genres,
		authors,
		studio,
		season_of,
		countries,
		seasons,
		quality_tag,
		extra: extra_extras(doc),
		..Default::default()
	}
}

/// The `extra` map a detail page hands over for free.
///
/// The page's own "Gợi ý cùng người xem" rail is a full set of cards sitting
/// right there in the markup we just parsed. The app re-queries
/// `get_recommended_anime` every time a new anime is opened, so throwing this
/// away means re-fetching a page we already have — the cards are serialised into
/// [EXTRA_RECOMMENDATIONS] instead and read back with no request at all.
///
/// The key is namespaced by source because `extra` is one flat namespace shared
/// by every source; an unprefixed key would let one source shadow another's.
fn extra_extras(doc: &Document) -> HashMap<String, String> {
	let mut extra = HashMap::new();
	let recommendations = parse_recommendations(doc);
	if !recommendations.is_empty() {
		// `Anime` is `serde::Serialize`, so the Lite cards go in as they are. A
		// failure is dropped rather than raised: a page that cannot be stashed is
		// still a perfectly good detail page, it just falls back to a re-fetch.
		if let Ok(json) = serde_json::to_string(&recommendations) {
			extra.insert(EXTRA_RECOMMENDATIONS.into(), json);
		}
	}
	extra
}

/// The detail page's own recommendation rail, as Lite cards, in site order.
///
/// `.MovieListRelated` is this site's "suggested for people who watched this"
/// block. The heading is matched as well as the class, so a deployment that
/// renames the wrapper but keeps the heading still resolves.
pub fn parse_recommendations(doc: &Document) -> Vec<Anime> {
	parse_cards(doc, RECOMMENDATION_RAIL)
}

/// The `extra` key the recommendations are stored under.
pub const EXTRA_RECOMMENDATIONS: &str = "avs.recommendations";

/// The recommendation rail on a detail page.
pub const RECOMMENDATION_RAIL: &str = ".MovieListRelated .TPostMv";

/// The recommendations stashed on `anime` by [extra_extras], or an empty list
/// when the page carried none.
///
/// This is the read half of [EXTRA_RECOMMENDATIONS], called by
/// `get_recommended_anime` with whatever anime the app already holds.
pub fn recommendations_from_extra(anime: &Anime) -> Vec<Anime> {
	anime
		.extra
		.get(EXTRA_RECOMMENDATIONS)
		.and_then(|json| serde_json::from_str(json).ok())
		.unwrap_or_default()
}

/// The labelled `.InfoList` rows of one metadata column.
struct InfoList {
	rows: Vec<(String, Element)>,
}

impl InfoList {
	/// Every `.AAIco-adjust` row of a column, paired with its `<strong>` label —
	/// lower-cased and stripped of its trailing colon.
	fn in_column(doc: &Document, column: &str) -> Self {
		let rows = doc
			.select(format!(".{column} .InfoList .AAIco-adjust"))
			.map(|list| {
				list.filter_map(|row| {
					let label = row
						.select_first("strong")
						.and_then(|e| e.text())
						.map(|t| collapse(&t).to_lowercase())
						.map(|t| t.trim_end_matches(':').trim().to_string())?;
					Some((label, row))
				})
				.collect()
			})
			.unwrap_or_default();
		Self { rows }
	}

	/// The anchors of the row whose label starts with `prefix`, as
	/// (text, last path segment).
	fn links(&self, prefix: &str) -> Vec<(String, String)> {
		let Some((_, row)) = self
			.rows
			.iter()
			.find(|(label, _)| label.starts_with(prefix))
		else {
			return Vec::new();
		};
		row.select("a[href]")
			.map(|list| {
				list.filter_map(|a| {
					let href = a.attr("href")?;
					// A studio the site has not filled in yet reads `Đang Cập Nhật`;
					// that is a placeholder, not a name, so it is dropped.
					let name = a
						.text()
						.map(|t| studio_name(&collapse(&t)))
						.filter(|n| !n.is_empty())?;
					// `/studio/CloverWorks.html` -> `CloverWorks`
					let segment = path_of(&href)
						.trim_end_matches(".html")
						.rsplit('/')
						.find(|s| !s.is_empty())?
						.to_string();
					Some((name, segment))
				})
				.collect()
			})
			.unwrap_or_default()
	}

	/// The row's anchors as display-only category links.
	fn plain_links(&self, prefix: &str) -> Vec<CategoryLink> {
		self.links(prefix)
			.into_iter()
			.map(|(name, _)| CategoryLink {
				name,
				filters: Vec::new(),
			})
			.collect()
	}

	/// The row's anchors as filter links, applying `slot` as the filter id.
	fn filtered_links(&self, prefix: &str, slot: &str) -> Vec<(String, Vec<FilterValue>)> {
		self.links(prefix)
			.into_iter()
			.map(|(name, value)| {
				(
					name,
					alloc::vec![FilterValue::Select {
						id: String::from(slot),
						value,
					}],
				)
			})
			.collect()
	}
}

/// The full ordered episode list from `/phim/{id}/xem-phim.html`.
///
/// Each anchor's `data-id` is what `POST /ajax/player` wants and its
/// `data-hash` is the per-episode signature that must be posted with it; the two
/// together are the whole handle for a stream. The visible text is the
/// zero-padded number (`01`), so the number is read from there.
pub fn parse_episodes(doc: &Document) -> Vec<Episode> {
	doc.select(EPISODES)
		.map(|list| {
			list.filter_map(|a| {
				let id = a.attr("data-id")?.trim().to_string();
				let hash = a.attr("data-hash")?.trim().to_string();
				if id.is_empty() || hash.is_empty() {
					return None;
				}
				let raw = a.text().map(|t| collapse(&t)).unwrap_or_default();
				let number = normalise_number(&raw);
				// The hash travels in the key: `get_stream` splits it back out.
				Some(Episode {
					key: format!("{number}-{id}-{hash}"),
					episode_number: number,
					locked: false,
					..Default::default()
				})
			})
			.collect()
		})
		.unwrap_or_default()
}

/// The `#filter` panel of `/danh-sach/all/`, read live.
///
/// Each group is a `.fc-*` block whose `<input name>` is the `/danh-sach/` slot
/// it drives (`type`, `season`, `genres[]`, `year`, `studio`, `rating`,
/// `country`), whose `<input value>` is the path segment, and whose wrapper
/// `<li>` carries the human label with a trailing count. `checkbox` means the
/// group allows multiple selections. `.fc-main` is the sort list instead — links
/// carrying `?sort=`, not inputs.
pub fn parse_filters(doc: &Document) -> Vec<Filter> {
	let Some(panel) = doc.select_first(FILTER_PANEL) else {
		return fallback_filters();
	};
	let mut out: Vec<Filter> = Vec::new();

	if let Some(sort) = parse_sort_filter(&panel) {
		out.push(sort);
	}

	// Order matters only for display, but keeping the site's own order makes
	// the panel read the way the website does.
	const GROUPS: [&str; 7] = [
		".fc-filmtype",
		".fc-quality",
		".fc-genre",
		".fc-release",
		".fc-studio",
		".fc-rating",
		".fc-country",
	];
	for selector in GROUPS {
		let Some(block) = panel.select_first(selector) else {
			continue;
		};
		let inputs: Vec<Element> = block
			.select("input")
			.map(|l| l.collect())
			.unwrap_or_default();
		let Some(first) = inputs.first() else {
			continue;
		};
		// `genres[]` is the site's plural spelling; SDK filter ids carry no
		// brackets, and `build_category_path` matches on the bare name.
		let id = first
			.attr("name")
			.map(|n| normalise_filter_id(&n))
			.unwrap_or_default();
		if id.is_empty() {
			continue;
		}
		let multiple = first.attr("type").as_deref() == Some("checkbox");
		let title = block
			.select_first(".fc-title")
			.and_then(|e| e.text())
			.map(|t| collapse(&t))
			.filter(|t| !t.is_empty())
			.unwrap_or_else(|| id.clone());

		let mut ids: Vec<Cow<'static, str>> = Vec::with_capacity(inputs.len());
		let mut names: Vec<Cow<'static, str>> = Vec::with_capacity(inputs.len());
		for input in &inputs {
			let value = input.attr("value").unwrap_or_default();
			if value.is_empty() {
				continue;
			}
			// The label sits on the input's wrapper, suffixed with a count
			// (`J.C.Staff (199)`); fall back to the raw value when absent.
			let label = input
				.parent()
				.and_then(|li| li.text())
				.map(|t| strip_count(&collapse(&t)))
				.filter(|t| !t.is_empty())
				.unwrap_or_else(|| value.clone());
			ids.push(Cow::Owned(value));
			names.push(Cow::Owned(label));
		}
		if names.is_empty() {
			continue;
		}
		// Only ship `ids` when they actually differ from the labels, which keeps
		// the common "label == value" groups compact.
		let same = ids.iter().zip(&names).all(|(a, b)| a == b);
		let option_ids = if same { None } else { Some(ids) };

		out.push(if multiple {
			MultiSelectFilter {
				id: Cow::Owned(id),
				title: Some(Cow::Owned(title)),
				can_exclude: true,
				uses_tag_style: true,
				options: names,
				ids: option_ids,
				..Default::default()
			}
			.into()
		} else {
			SelectFilter {
				id: Cow::Owned(id),
				title: Some(Cow::Owned(title)),
				options: names,
				ids: option_ids,
				..Default::default()
			}
			.into()
		});
	}

	if out.is_empty() {
		fallback_filters()
	} else {
		out
	}
}

/// The `.fc-main` block — the site's sort list, which is a set of links rather
/// than inputs. The first entry is the unfiltered default and gets an empty id
/// so that leaving it alone does not add a `?sort=`.
fn parse_sort_filter(panel: &Element) -> Option<Filter> {
	let links: Vec<Element> = panel
		.select(".fc-main li a")
		.map(|l| l.collect())
		.unwrap_or_default();
	let mut names: Vec<Cow<'static, str>> = Vec::new();
	let mut ids: Vec<Cow<'static, str>> = Vec::new();
	for (index, a) in links.iter().enumerate() {
		let name = a.text().map(|t| collapse(&t)).filter(|t| !t.is_empty())?;
		let value = a
			.attr("href")
			.and_then(|href| sort_value(&href))
			.unwrap_or_default();
		names.push(Cow::Owned(name));
		ids.push(Cow::Owned(if index == 0 { String::new() } else { value }));
	}
	if names.is_empty() {
		return None;
	}
	Some(
		SelectFilter {
			id: Cow::Borrowed("sort"),
			title: Some(Cow::Borrowed("Sắp xếp theo")),
			options: names,
			ids: Some(ids),
			..Default::default()
		}
		.into(),
	)
}

/// A filter set that does not need the site to answer, used when the panel
/// cannot be read — a challenge page, or the site dropping a group.
fn fallback_filters() -> Vec<Filter> {
	const SORTS: [(&str, &str); 6] = [
		("", "Mới nhất"),
		("latest", "Mới nhất"),
		("nameaz", "Tên A → Z"),
		("nameza", "Tên Z → A"),
		("view", "Lượt xem"),
		("rating", "Đánh giá"),
	];
	const TYPES: [(&str, &str); 5] = [
		("all", "Tất cả"),
		("list-le", "Anime lẻ (Movie/OVA)"),
		("list-bo", "Anime bộ (TV-Series)"),
		("list-tron-bo", "Anime trọn bộ"),
		("list-dang-chieu", "Đang chiếu"),
	];
	fn pairs(table: &[(&str, &str)]) -> (Vec<Cow<'static, str>>, Vec<Cow<'static, str>>) {
		(
			table.iter().map(|(_, n)| Cow::Owned((*n).into())).collect(),
			table.iter().map(|(v, _)| Cow::Owned((*v).into())).collect(),
		)
	}
	let (sort_names, sort_ids) = pairs(&SORTS);
	let (type_names, type_ids) = pairs(&TYPES);
	vec![
		SelectFilter {
			id: Cow::Borrowed("sort"),
			title: Some(Cow::Borrowed("Sắp xếp theo")),
			options: sort_names,
			ids: Some(sort_ids),
			..Default::default()
		}
		.into(),
		SelectFilter {
			id: Cow::Borrowed(FILTER_TYPE),
			title: Some(Cow::Borrowed("Loại")),
			options: type_names,
			ids: Some(type_ids),
			..Default::default()
		}
		.into(),
	]
}

/// Build the `/danh-sach/{type}/{slots…}/` path (plus `?sort=`) from the active
/// filter values.
///
/// The site addresses filter combinations *positionally*, in exactly the order
/// of [`CATEGORY_SLOTS`], spelling an unused slot `all`. Genres are the one
/// multi-valued slot: they join with `-`, and an excluded one is prefixed `!`.
pub fn build_category_path(filters: &[FilterValue], page: i32) -> String {
	let mut path = String::from("/danh-sach/");

	// The first segment is the catalogue, not one of the six slots.
	let root = select_value(filters, FILTER_TYPE)
		.filter(|v| v != SLOT_ANY)
		.unwrap_or_else(|| SLOT_ANY.to_string());
	push_segment(&mut path, &root);

	for slot in CATEGORY_SLOTS {
		if slot == FILTER_GENRES {
			let joined = multi_values(filters, FILTER_GENRES)
				.into_iter()
				.map(
					|(value, excluded)| {
						if excluded { format!("!{value}") } else { value }
					},
				)
				.collect::<Vec<_>>()
				.join("-");
			push_segment(
				&mut path,
				if joined.is_empty() { SLOT_ANY } else { &joined },
			);
		} else {
			let value = select_value(filters, slot).unwrap_or_else(|| SLOT_ANY.to_string());
			push_segment(&mut path, &value);
		}
	}

	// Every slot path ends in a slash; `trang-N` and `?sort=` hang off that.
	if !path.ends_with('/') {
		path.push('/');
	}
	if page > 1 {
		path.push_str("trang-");
		path.push_str(&page.to_string());
		path.push('/');
	}
	// Sort rides in the query, and only when the user moved off the default.
	if let Some(sort) = select_value(filters, "sort").filter(|v| !v.is_empty()) {
		path.push_str("?sort=");
		path.push_str(&encode_uri_component(sort.as_str()));
	}
	path
}

/// The search path: `/tim-kiem/{keyword}/`, with `trang-{n}/` past the first
/// page. The keyword is one whole path segment, spaces included.
pub fn build_search_path(keyword: &str, page: i32) -> String {
	let mut path = String::from("/tim-kiem/");
	path.push_str(&encode_uri_component(keyword));
	path.push('/');
	if page > 1 {
		path.push_str("trang-");
		path.push_str(&page.to_string());
		path.push('/');
	}
	path
}

// ── filter value helpers ───────────────────────────────────────────────────

/// The chosen value of a single-select filter, ignoring the `all` sentinel.
fn select_value(filters: &[FilterValue], id: &str) -> Option<String> {
	filters.iter().find_map(|f| match f {
		FilterValue::Select { id: fid, value } if fid == id => Some(value.clone()),
		_ => None,
	})
}

/// Every value of a multi-select filter, paired with its exclusion flag.
fn multi_values(filters: &[FilterValue], id: &str) -> Vec<(String, bool)> {
	filters
		.iter()
		.flat_map(|f| match f {
			FilterValue::MultiSelect {
				id: fid,
				included,
				excluded,
			} if fid == id => included
				.iter()
				.map(|v| (v.clone(), false))
				.chain(excluded.iter().map(|v| (v.clone(), true)))
				.collect::<Vec<_>>(),
			_ => Vec::new(),
		})
		.filter(|(v, _)| !v.is_empty() && v != SLOT_ANY)
		.collect()
}

/// Append a path segment, percent-encoding what the site cares about — studio
/// names such as `J.C.Staff` and the `!` exclusion marker.
fn push_segment(path: &mut String, segment: &str) {
	if !path.ends_with('/') {
		path.push('/');
	}
	path.push_str(&encode_uri_component(segment));
}

// ── small helpers ──────────────────────────────────────────────────────────

/// A poster url, preferring the Cloudflare original.
///
/// `data-cfsrc` is Cloudflare's opt-in attribute for the pre-optimisation url,
/// which keeps full resolution. The site does not currently emit it on the
/// card grids, so this is a preference rather than a requirement — `src` is the
/// normal path and the fallback covers lazy-loading attributes too.
pub fn image_url(scope: &Element) -> String {
	scope
		.select_first("img")
		.and_then(|img| {
			img.attr("data-cfsrc")
				.filter(|s| !s.is_empty())
				.or_else(|| img.attr("src").filter(|s| !s.is_empty()))
				.or_else(|| img.attr("data-src").filter(|s| !s.is_empty()))
		})
		.map(|s| absolute(&s))
		.unwrap_or_default()
}

/// The key of a `/phim/{slug}-a{id}/…` url — the slug segment, which is both the
/// anime key and the prefix of the detail, watch and episode urls.
///
/// Only the *first* segment counts: a deep link to
/// `/phim/{slug}/tap-11-115905.html` must resolve to the same anime as the bare
/// `/phim/{slug}/`.
pub fn anime_key_of(href: &str) -> Option<&str> {
	let rest = path_of(href).strip_prefix("/phim/")?;
	let key = rest.split('/').next().unwrap_or("").trim();
	if key.is_empty() { None } else { Some(key) }
}

/// The path of `href`, absolute or relative, without query or fragment.
pub fn path_of(href: &str) -> &str {
	let rest = match href.find("://") {
		Some(i) => match href[i + 3..].find('/') {
			Some(slash) => &href[i + 3 + slash..],
			None => "/",
		},
		None => href,
	};
	match rest.find(['?', '#']) {
		Some(cut) => &rest[..cut],
		None => rest,
	}
}

/// Point a possibly-relative url at the base host.
pub fn absolute(url: &str) -> String {
	if url.is_empty() || url.starts_with("http://") || url.starts_with("https://") {
		url.to_string()
	} else if let Some(rest) = url.strip_prefix("//") {
		format!("https://{rest}")
	} else if url.starts_with('/') {
		format!("{}{url}", crate::catalog::DEFAULT_BASE)
	} else {
		url.to_string()
	}
}

/// `9.7 trong số 10`, `9.7` → `9.7`.
pub fn parse_rating(text: &str) -> Option<f32> {
	let number: String = text
		.trim_start()
		.chars()
		.take_while(|c| c.is_ascii_digit() || *c == '.')
		.collect();
	// A run of only dots is not a number.
	if number.is_empty() || number.chars().all(|c| c == '.') {
		return None;
	}
	number.parse().ok()
}

/// The leading integer of `text`, used for years and counts.
pub fn first_number(text: &str) -> Option<i32> {
	let digits: String = text
		.trim_start()
		.chars()
		.skip_while(|c| !c.is_ascii_digit())
		.take_while(|c| c.is_ascii_digit())
		.collect();
	digits.parse().ok()
}

/// `1179 tập`, `TẬP 12` → the number.
pub fn parse_number(text: &str) -> Option<f64> {
	let digits: String = text
		.chars()
		.skip_while(|c| !c.is_ascii_digit() && *c != '.')
		.take_while(|c| c.is_ascii_digit() || *c == '.')
		.collect();
	if digits.is_empty() || digits.chars().all(|c| c == '.') {
		return None;
	}
	digits.parse().ok()
}

/// `Studio: CloverWorks` → `CloverWorks`.
///
/// The label sits in a leading `<span>`, and a studio the site has not filled in
/// yet reads `Đang Cập Nhật` / `Đoán xem` — both are dropped rather than shown
/// as a studio name.
fn studio_name(text: &str) -> String {
	let name = match text.split_once(':') {
		Some((_, tail)) => tail.trim(),
		None => text.trim(),
	};
	let lower = name.to_lowercase();
	if lower.contains("dang cap nhat")
		|| lower.contains("đang cập nhật")
		|| lower.contains("doan xem")
		|| lower.contains("đoán xem")
	{
		return String::new();
	}
	name.to_string()
}

/// `Lượt xem: 7,524,686` → `7524686`.
///
/// The thousands separators are stripped, and only the digits *after* the colon
/// are considered — the label itself starts with `Lượt`, but a bare `1.234` must
/// not be read as `1`.
fn parse_count(text: &str) -> i32 {
	let digits: String = text
		.split_once(':')
		.map(|(_, tail)| tail)
		.unwrap_or(text)
		.chars()
		.filter(|c| c.is_ascii_digit())
		.collect();
	digits.parse().unwrap_or(0)
}

/// `11/12` → `(Some(11.0), Some(12))`.
fn split_progress(text: &str) -> (Option<f64>, Option<i32>) {
	let flat = collapse(text);
	let mut parts = flat.splitn(2, '/');
	(
		parts.next().and_then(parse_number),
		parts.next().and_then(first_number),
	)
}

/// A trailing `/total`, e.g. `12/24`.
fn parse_total(text: &str) -> Option<i32> {
	text.split('/').nth(1).and_then(first_number)
}

/// The episode number of a list entry, keeping halves (`12.5`) intact.
///
/// Kept in string space on purpose: the site writes `01`, `12`, `12.5` and the
/// occasional `OVA`, and rounding a half-episode through `f64` is how `12.5`
/// turns into `13`.
pub fn normalise_number(raw: &str) -> String {
	let digits = extract_number(raw.trim());
	if digits.is_empty() {
		return raw.trim().to_string();
	}
	// Drop a redundant trailing `.0` so a key reads `1-114607`, not `1.0-…`.
	let digits = digits.strip_suffix(".0").unwrap_or(digits);
	// The site pads its episode numbers (`01`), which would make the key and the
	// display value disagree. Strip the padding from the integer half only, so
	// `12.5` keeps its fraction and `0` stays `0`.
	let (integer, fraction) = match digits.split_once('.') {
		Some((head, tail)) => (head, Some(tail)),
		None => (digits, None),
	};
	let stripped = integer.trim_start_matches('0');
	let integer = if stripped.is_empty() { "0" } else { stripped };
	match fraction {
		Some(tail) => format!("{integer}.{tail}"),
		None => integer.to_string(),
	}
}

/// The leading numeric run of `text`, as written.
fn extract_number(text: &str) -> &str {
	let start = text
		.char_indices()
		.find(|(_, c)| c.is_ascii_digit())
		.map(|(i, _)| i)
		.unwrap_or(text.len());
	let rest = &text[start..];
	let end = rest
		.char_indices()
		.find(|(_, c)| !(c.is_ascii_digit() || *c == '.'))
		.map(|(i, _)| i)
		.unwrap_or(rest.len());
	&rest[..end]
}

/// The site prefixes the synopsis with the episode it describes: `[Tập 11] `.
fn strip_episode_marker(text: &str) -> String {
	let trimmed = text.trim_start();
	let Some(rest) = trimmed.strip_prefix('[') else {
		return trimmed.to_string();
	};
	let Some(end) = rest.find(']') else {
		return trimmed.to_string();
	};
	let inner = &rest[..end];
	let lower = inner.to_lowercase();
	if lower.contains("tập") || lower.contains("tap") {
		rest[end + 1..].trim_start().to_string()
	} else {
		trimmed.to_string()
	}
}

/// `J.C.Staff (199)` → `J.C.Staff`.
fn strip_count(label: &str) -> String {
	let trimmed = label.trim();
	if !trimmed.ends_with(')') {
		return trimmed.to_string();
	}
	match trimmed.rfind('(') {
		Some(open)
			if trimmed[open + 1..trimmed.len() - 1]
				.chars()
				.all(|c| c.is_ascii_digit()) =>
		{
			trimmed[..open].trim_end().to_string()
		}
		_ => trimmed.to_string(),
	}
}

/// `genres[]` → `genres`; SDK filter ids carry no brackets.
pub fn normalise_filter_id(raw: &str) -> String {
	raw.replace("[]", "").trim().to_string()
}

/// The `sort=` value inside a filter link.
fn sort_value(href: &str) -> Option<String> {
	let query = href.split_once('?')?.1;
	query
		.split('&')
		.find_map(|pair| pair.strip_prefix("sort="))
		.map(|v| v.to_string())
}

/// Collapse runs of whitespace and trim — the site's markup wraps Vietnamese
/// text across lines constantly.
pub fn collapse(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut space = false;
	for ch in text.chars() {
		if ch.is_whitespace() {
			space = true;
			continue;
		}
		if space && !out.is_empty() {
			out.push(' ');
		}
		space = false;
		out.push(ch);
	}
	out
}

/// The listing ids this source serves, paired with their labels.
pub fn static_listings() -> Vec<Listing> {
	LISTINGS
		.iter()
		.map(|(id, name)| Listing {
			id: (*id).into(),
			name: (*name).into(),
			kind: ListingKind::List,
		})
		.chain(RANKING_TYPES.iter().map(|(id, name)| Listing {
			id: format!("bang-xep-hang/{id}.html"),
			name: (*name).into(),
			kind: ListingKind::List,
		}))
		.collect()
}
