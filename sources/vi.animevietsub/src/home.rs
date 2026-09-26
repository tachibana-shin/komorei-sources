//! The home layout, mirroring the site's own six rails.
//!
//! Each rail's selector was confirmed against the live front page; the counts
//! in the comments are what the site returned when this source was written, and
//! are here to make a structural change to the markup obvious.

extern crate alloc;

use alloc::{format, vec::Vec};

use komorei::{
	Anime, HomeComponent, HomeComponentValue, HomeLayout, Link, LinkValue, Listing, ListingKind,
	Result, imports::html::Document,
};

use crate::{catalog, net::fetch_html, parsers};

/// One rail of the home page.
struct Rail {
	title: &'static str,
	selector: &'static str,
	/// `true` for the wide banner carousel, `false` for a poster rail.
	featured: bool,
}

const RAILS: [Rail; 6] = [
	Rail {
		title: "Nổi bật",
		selector: ".MovieListTopCn .TPostMv", // 16 cards
		featured: false,
	},
	Rail {
		title: "AnimeVietSub chọn",
		selector: ".MovieListSldCn .TPostMv", // 10 cards, the wide carousel
		featured: true,
	},
	Rail {
		title: "Mới cập nhật",
		selector: "#single-home .TPostMv", // 10 cards
		featured: false,
	},
	Rail {
		title: "Tiền chiếu",
		selector: "#new-home .TPostMv", // 10 cards
		featured: false,
	},
	Rail {
		title: "Đề cửa",
		selector: "#hot-home .TPostMv", // 10 cards
		featured: false,
	},
	Rail {
		title: "Top phim",
		// This rail is not a `.TPostMv` block: it is a `ul.MovieList` of plain
		// `li > .TPost.A` rows, each carrying a `.Top` rank badge. A
		// `.TPostMv` selector here matches nothing.
		selector: "#showTopPhim .TPost", // 5 rows
		featured: false,
	},
];

/// Build the home layout from the site's front page.
pub fn build(base: &str) -> Result<HomeLayout> {
	let doc = fetch_html(&format!("{base}/"))?;
	Ok(from_document(&doc))
}

/// The layout half of [`build`], split out so it can be exercised on a fixture.
///
/// A rail that comes back empty is dropped rather than rendered as a blank
/// strip: the site's markup shifts between deployments, and a section that
/// disappears upstream should disappear here too.
pub fn from_document(doc: &Document) -> HomeLayout {
	let mut components: Vec<HomeComponent> = Vec::new();

	for rail in RAILS {
		let entries: Vec<Anime> = parsers::parse_cards(doc, rail.selector);
		if entries.is_empty() {
			continue;
		}
		let value = if rail.featured {
			HomeComponentValue::BigScroller {
				entries,
				auto_scroll_interval: Some(5.0),
			}
		} else {
			HomeComponentValue::Scroller {
				entries: entries.into_iter().map(Link::from).collect(),
				listing: None,
			}
		};
		components.push(HomeComponent {
			title: Some(rail.title.into()),
			subtitle: None,
			value,
		});
	}

	// A "see all" strip over the full catalogue, the way the site's own section
	// headers do.
	if !components.is_empty() {
		components.push(HomeComponent {
			title: Some("Danh sách".into()),
			subtitle: None,
			value: HomeComponentValue::Links(
				catalog::LISTINGS
					.iter()
					.map(|(id, name)| Listing {
						id: (*id).into(),
						name: (*name).into(),
						kind: ListingKind::List,
					})
					.map(|listing| {
						let title = listing.name.clone();
						Link {
							title,
							value: Some(LinkValue::Listing(listing)),
							..Default::default()
						}
					})
					.collect(),
			),
		});
	}

	HomeLayout { components }
}
