//! Site constants: the base host, the listing catalogue, and the fixed filter
//! slots the site's `/danh-sach/…` paths are built from.
//!
//! Everything here was read off the live site. The genre/country/year/studio
//! taxonomies are deliberately **not** hard-coded: they live in the `#filter`
//! block of `/danh-sach/all/` and are read at runtime by
//! [`crate::parsers::parse_filters`], because the site changes them (it already
//! carries a `None found, add some` studio and a `None` rating bucket).

/// The current host, taken from the project's own domain-resolver payload
/// (`transform.json`). The site is behind a rotating set of mirrors; the
/// resolver is an app-level concern, so this source just ships the newest
/// advertised host and lets the `base_url` setting override it.
pub const DEFAULT_BASE: &str = "https://animevietsub.li";

/// The `base_url` source setting key.
pub const SETTING_BASE_URL: &str = "base_url";
/// A notification key emitted after the base url changes.
pub const NOTIFICATION_BASE_URL: &str = "base_url_changed";

/// The site's own brand, used in the filter note.
pub const SITE_NAME: &str = "AnimeVietsub";

/// The `type` filter — which catalogue a `/danh-sach/…` path is rooted at.
pub const FILTER_TYPE: &str = "type";
/// Multi-value genre slot; the site names it with a `[]` suffix.
pub const FILTER_GENRES: &str = "genres";
pub const FILTER_SEASON: &str = "season";
pub const FILTER_YEAR: &str = "year";
pub const FILTER_STUDIO: &str = "studio";
pub const FILTER_RATING: &str = "rating";
pub const FILTER_COUNTRY: &str = "country";

/// The ordered slots of a `/danh-sach/…` path. The order is the site's, not
/// ours — `type` first, then these six, each falling back to `all`.
pub const CATEGORY_SLOTS: [&str; 6] = [
	FILTER_GENRES,
	FILTER_SEASON,
	FILTER_YEAR,
	FILTER_STUDIO,
	FILTER_RATING,
	FILTER_COUNTRY,
];

/// The path segment that means "no restriction" in every slot.
pub const SLOT_ANY: &str = "all";

/// Ranking boards, keyed by the site's own path segment. The five boards are
/// exactly the ones the site links from its nav; `voted` is the "most voted"
/// board, which carries no Vietnamese label of its own.
pub const RANKING_TYPES: [(&str, &str); 5] = [
	("day", "Top ngày"),
	("voted", "Bình chọn nhiều"),
	("month", "Top tháng"),
	("season", "Top mùa"),
	("year", "Top năm"),
];

/// The site's static listing pages that are not filters — each id is the path
/// relative to the base url.
pub const LISTINGS: [(&str, &str); 8] = [
	("anime-moi/", "Anime mới"),
	("anime-sap-chieu/", "Đang chiếu"),
	("anime-bo/", "Anime bộ"),
	("anime-le/", "Anime lẻ"),
	("danh-sach/list-dang-chieu/", "Danh sách đang chiếu"),
	("danh-sach/list-tron-bo/", "Danh sách trọn bộ"),
	("hoat-hinh-trung-quoc/", "Hoạt hình Trung Quốc"),
	("anime/library/0-9/", "A – Z"),
];

/// True when `kind` names one of the site's ranking boards.
pub fn is_ranking(kind: &str) -> bool {
	RANKING_TYPES.iter().any(|(id, _)| *id == kind)
}
