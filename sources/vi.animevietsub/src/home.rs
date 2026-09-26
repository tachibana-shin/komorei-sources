//! The home layout, mirroring the site's own six rails plus a launcher strip.
//!
//! Each rail's selector was confirmed against the live front page; the counts in
//! the comments are what the site returned when this source was written, and are
//! here to make a structural change to the markup obvious.
//!
//! The layout deliberately mixes component kinds — a filter launcher, the wide
//! carousel, poster rails, a numbered ranking and a page list — because six
//! identical poster rails read as one long undifferentiated list. Every rail
//! that has a matching catalogue page carries its [`Listing`], which is what
//! renders the row's "see all" affordance.

extern crate alloc;

use alloc::{
	format,
	string::{String, ToString},
	vec::Vec,
};

use komorei::{
	Anime, FilterItem, FilterValue, HomeComponent, HomeComponentValue, HomeLayout, Link, LinkValue,
	Listing, ListingKind, Result, imports::html::Document,
};

use crate::{
	catalog::{FILTER_COUNTRY, FILTER_SEASON, FILTER_TYPE, LISTINGS, RANKING_TYPES},
	net::fetch_html,
	parsers,
};

/// One rail of the home page.
struct Rail {
	title: &'static str,
	subtitle: Option<&'static str>,
	selector: &'static str,
	/// The catalogue page this rail previews, for its "see all" link.
	listing: &'static str,
	/// How the rail is presented.
	kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
	/// A horizontal poster rail.
	Scroller,
	/// The site's wide banner carousel.
	Banner,
	/// A numbered list — the site renders real rank badges in this rail.
	Ranking,
}

const RAILS: [Rail; 6] = [
	Rail {
		title: "AnimeVietSub chọn",
		subtitle: None,
		selector: ".MovieListSldCn .TPostMv", // 10 wide cards
		listing: "",
		kind: Kind::Banner,
	},
	Rail {
		title: "Nổi bật",
		subtitle: None,
		selector: ".MovieListTopCn .TPostMv", // 16 cards
		listing: "anime-moi/",
		kind: Kind::Scroller,
	},
	Rail {
		title: "Mới cập nhật",
		subtitle: None,
		selector: "#single-home .TPostMv", // 10 cards
		listing: "anime-moi/",
		kind: Kind::Scroller,
	},
	Rail {
		title: "Đề cửa",
		subtitle: None,
		selector: "#hot-home .TPostMv", // 10 cards
		listing: "",
		kind: Kind::Scroller,
	},
	Rail {
		title: "Tiền chiếu",
		subtitle: Some("Sắp chiếu"),
		selector: "#new-home .TPostMv", // 10 cards
		listing: "anime-sap-chieu/",
		kind: Kind::Scroller,
	},
	Rail {
		title: "Top phim tuần",
		subtitle: None,
		// Not a `.TPostMv` block: a `ul.MovieList` of `li > .TPost.A` rows, each
		// carrying a `.Top` rank badge. A `.TPostMv` selector matches nothing.
		selector: "#showTopPhim .TPost",
		// The site has no `/bang-xep-hang/week.html` — its nav carries
		// day/voted/month/season/year — so this rail has no "see all" target.
		listing: "",
		kind: Kind::Ranking,
	},
];

/// Build the home layout from the site's front page.
pub fn build(base: &str) -> Result<HomeLayout> {
	let doc = fetch_html(&format!("{base}/"))?;
	let mut layout = from_document(&doc);
	// The ranking boards are a second request. Appending them after the layout is
	// already assembled means the first paint does not wait for them.
	if let Some(board) = fetch_ranking(base) {
		layout.components.push(board);
	}
	Ok(layout)
}

/// The layout half of [`build`], split out so it can be exercised on a fixture.
///
/// A rail that comes back empty is dropped rather than rendered as a blank
/// strip: the site's markup shifts between deployments, and a section that
/// disappears upstream should disappear here too.
pub fn from_document(doc: &Document) -> HomeLayout {
	let mut components: Vec<HomeComponent> = Vec::new();

	// A launcher strip first, the way the website's own nav sits above the
	// content. These ids are the static ones the catalogue paths take
	// (`/danh-sach/{type}/{genres}/{season}/{year}/…`), so the shortcuts resolve
	// without a round trip to the filter panel.
	components.push(launcher_strip());

	for rail in RAILS {
		let entries: Vec<Anime> = parsers::parse_cards(doc, rail.selector);
		if entries.is_empty() {
			continue;
		}
		let listing = (!rail.listing.is_empty()).then(|| listing(rail.listing));
		let value = match rail.kind {
			Kind::Banner => HomeComponentValue::BigScroller {
				entries,
				auto_scroll_interval: Some(5.0),
			},
			Kind::Scroller => HomeComponentValue::Scroller {
				entries: entries.into_iter().map(Link::from).collect(),
				listing,
			},
			// The site prints `#1`, `#2`, … in this rail's badges, so the list
			// is presented as a ranking rather than another poster row.
			Kind::Ranking => HomeComponentValue::AnimeList {
				ranking: true,
				page_size: Some(10),
				entries: entries.into_iter().map(Link::from).collect(),
				listing,
			},
		};
		components.push(HomeComponent {
			title: Some(rail.title.into()),
			subtitle: rail.subtitle.map(Into::into),
			value,
		});
	}

	// A "see all" strip over the full catalogue, the way the site's own section
	// headers do.
	components.push(catalogue_links());

	HomeLayout { components }
}

/// A `Filters` strip of one-tap catalogue shortcuts.
///
/// Only slots whose values are stable and readable from a url are used here:
/// `type`, `season` and `country`. Genres are deliberately left out — the site
/// addresses them by *numeric* id (`genres[]` carries `value="1"`), which is
/// only knowable from the live `#filter` panel, and guessing it would produce
/// silently wrong results.
fn launcher_strip() -> HomeComponent {
	let select = |id: &str, name: &str, value: &str| FilterItem {
		title: name.into(),
		values: Some(alloc::vec![FilterValue::Select {
			id: id.into(),
			value: value.into(),
		}]),
	};
	HomeComponent {
		title: Some("Duyệt nhanh".into()),
		subtitle: None,
		value: HomeComponentValue::Filters(alloc::vec![
			select(FILTER_TYPE, "Tất cả", "all"),
			select(FILTER_TYPE, "Anime bộ", "list-bo"),
			select(FILTER_TYPE, "Anime lẻ", "list-le"),
			select(FILTER_TYPE, "Trọn bộ", "list-tron-bo"),
			select(FILTER_TYPE, "Đang chiếu", "list-dang-chieu"),
			select(FILTER_SEASON, "Mùa đông", "winter"),
			select(FILTER_SEASON, "Mùa xuân", "spring"),
			select(FILTER_SEASON, "Mùa hạ", "summer"),
			select(FILTER_SEASON, "Mùa thu", "autumn"),
			select(FILTER_COUNTRY, "Nhật Bản", "jp"),
			select(FILTER_COUNTRY, "Trung Quốc", "cn"),
		]),
	}
}

/// A `Links` strip over the static catalogue pages and the ranking boards.
fn catalogue_links() -> HomeComponent {
	let mut links: Vec<Link> = LISTINGS
		.iter()
		.map(|(id, name)| Link {
			title: (*name).into(),
			value: Some(LinkValue::Listing(listing(id))),
			..Default::default()
		})
		.chain(RANKING_TYPES.iter().map(|(id, name)| Link {
			title: (*name).into(),
			value: Some(LinkValue::Listing(listing(&format!(
				"bang-xep-hang/{id}.html"
			)))),
			..Default::default()
		}))
		.collect();
	// `Links` renders nothing for an empty list, so a caller cannot tell the
	// strip was dropped from a site that genuinely has no static pages.
	if links.is_empty() {
		links.push(Link {
			title: String::new(),
			..Default::default()
		});
	}
	HomeComponent {
		title: Some("Bảng xếp hạng".into()),
		subtitle: None,
		value: HomeComponentValue::Links(links),
	}
}

/// A ranking board as a numbered list component.
fn fetch_ranking(base: &str) -> Option<HomeComponent> {
	// `week` is not one of the site's boards; `voted` is its "most voted" one.
	let kind = RANKING_TYPES
		.iter()
		.map(|(id, _)| *id)
		.find(|id| *id == "voted")?;
	let path = format!("bang-xep-hang/{kind}.html");
	let doc = fetch_html(&format!("{base}/{path}")).ok()?;
	let entries = parsers::parse_ranking(&doc);
	if entries.is_empty() {
		return None;
	}
	Some(HomeComponent {
		title: Some("Bình chọn nhiều".into()),
		subtitle: None,
		value: HomeComponentValue::AnimeList {
			ranking: true,
			page_size: Some(10),
			entries: entries.into_iter().map(Link::from).collect(),
			listing: Some(listing(&path)),
		},
	})
}

/// A [`Listing`] for a catalogue path, defaulting to a plain list kind.
pub fn listing(id: &str) -> Listing {
	Listing {
		id: id.to_string(),
		name: String::new(),
		kind: ListingKind::List,
	}
}
