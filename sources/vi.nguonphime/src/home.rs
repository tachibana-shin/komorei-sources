//! Home layout — three parallel rails (Mới Cập Nhật / Phim Bộ / Phim Lẻ),
//! a featured banner, genre chips and quick links. Each request tolerates the
//! NP Checker bounce by retrying once after the session cookies land.

use alloc::{
	format,
	string::{String, ToString},
	vec,
	vec::Vec,
};
use komorei::{
	Anime, AnimeWithEpisode, FilterItem, FilterValue, Home, HomeComponent, HomeComponentValue,
	HomeLayout, Link, LinkValue, Listing, ListingKind,
	imports::html::Html,
	imports::net::Request,
};

use crate::NguonphimeSource;
use crate::catalog::{GENRES, LISTINGS};
use crate::parsers::{home_episode, parse_list_page};
use crate::util::is_checker_page;

impl Home for NguonphimeSource {
	fn get_home(&self) -> komorei::Result<HomeLayout> {
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
					page_size: Some(4),
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
					page_size: Some(4),
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