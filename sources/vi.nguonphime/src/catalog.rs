//! Static catalogs of the site: source constants, the filter genre/country/
//! type tables, the hardcoded playback-server list, and the dynamic listings.
//! Kept apart from the parsing code so a listing/filter change is a one-line
//! edit (values were taken from the site's own nav menu and breadcrumbs).

use alloc::{format, string::String, vec::Vec};

pub(crate) const SOURCE_ID: &str = "vi.nguonphime";
pub(crate) const DEFAULT_BASE: &str = "https://nguonphime.site";
pub(crate) const SETTING_BASE_URL: &str = "base_url";
pub(crate) const SETTING_LAST_NOTIFICATION: &str = "last_notification";

/// The stream grant always requests the plain unencrypted HLS format so the
/// app can play the result directly (no JS AES-GCM unwrapping in the player).
pub(crate) const PLAYLIST_FORMAT: &str = "hls";

/// Playback servers rendered by the grab player page (`li.serverItem
/// data-index`). Constant across the network; the season model maps one
/// Komorei season per server.
pub(crate) const SERVERS: &[&str] = &["PAI", "NGC"];

// ────────────────────────────────────────────────────────────────────────────
// Catalogs (name → URL slug → category id / filter code). All values were
// taken from the site's own nav menu and breadcrumb links.
// ────────────────────────────────────────────────────────────────────────────

/// Genre catalog: displayed name, URL slug, category id (`/phim-{slug}-c{id}.html`).
pub(crate) const GENRES: &[(&str, &str, &str)] = &[
	("Hành Động", "hanh-dong", "3"),
	("Võ Thuật", "vo-thuat", "4"),
	("Tâm Lý - Tình Cảm", "tam-ly-tinh-cam", "5"),
	("Hài Hước", "hai-huoc", "6"),
	("Hoạt Hình", "hoat-hinh", "7"),
	("Phiêu Lưu", "phieu-luu", "8"),
	("Kinh Dị", "kinh-di", "9"),
	("Hình Sự", "hinh-su", "10"),
	("Chiến Tranh", "chien-tranh", "11"),
	("Thần Thoại", "than-thoai", "12"),
	("Viễn Tưởng", "vien-tuong", "13"),
	("Cổ Trang", "co-trang", "14"),
	("Khoa Học Tài Liệu", "khoa-hoc-tai-lieu", "15"),
	("Âm Nhạc", "am-nhac", "16"),
	("Phim 18+", "18", "17"),
	("Chiếu Rạp", "chieu-rap", "18"),
	("Xã Hội Đen", "xa-hoi-den", "19"),
	("Việt Xưa", "viet-xua", "20"),
];

/// Country catalog: displayed name, URL slug, ISO code
/// (`/tuy-chon/{slug}.html?ft=co&co={code}`).
pub(crate) const COUNTRIES: &[(&str, &str, &str)] = &[
	("Trung Quốc", "trung-quoc", "CN"),
	("Hàn Quốc", "han-quoc", "KR"),
	("Nhật Bản", "nhat-ban", "JP"),
	("Thái Lan", "thai-lan", "TH"),
	("Việt Nam", "viet-nam", "VN"),
	("Hồng Kông", "hong-kong", "HK"),
	("Đài Loan", "dai-loan", "TW"),
	("Mỹ", "my", "US"),
	("Pháp", "phap", "FR"),
	("Anh", "anh", "GB"),
	("Ấn Độ", "an-do", "IN"),
	("Indonesia", "indonesia", "ID"),
	("Malaysia", "malaysia", "MY"),
	("Mexico", "mexico", "MX"),
	("Brazil", "brazil", "BR"),
	("Tây Ban Nha", "tay-ban-nha", "ES"),
	("Úc", "uc", "AU"),
];

/// "Loại phim" select + home rails: name, slug, filter key, filter value
/// (`/tuy-chon/{slug}.html?ft={key}&{key}={value}`).
pub(crate) const TYPE_FILTERS: &[(&str, &str, &str, &str)] = &[
	("Phim Hot", "phim-hot", "ho", "1"),
	("Phim Mới", "phim-moi", "ne", "1"),
	("Phim Lẻ", "phim-le", "ty", "1"),
	("Phim Bộ", "phim-bo", "ty", "2"),
];

pub(crate) const YEAR_FIRST: i32 = 1998;
pub(crate) const YEAR_LAST: i32 = 2026;

pub(crate) fn year_options() -> Vec<String> {
	(YEAR_FIRST..=YEAR_LAST).map(|y| format!("{y}")).collect()
}

/// Dynamic listings — each id IS the site's relative list path.
pub(crate) const LISTINGS: &[(&str, &str)] = &[
	("tuy-chon/phim-moi.html?ft=ne&ne=1", "Mới Cập Nhật"),
	("tuy-chon/phim-hot.html?ft=ho&ho=1", "Phim Hot"),
	("tuy-chon/phim-bo.html?ft=ty&ty=2", "Phim Bộ"),
	("tuy-chon/phim-le.html?ft=ty&ty=1", "Phim Lẻ"),
	("phim-chieu-rap-c18.html", "Chiếu Rạp"),
	("phim-hoat-hinh-c7.html", "Hoạt Hình"),
	("phim-hanh-dong-c3.html", "Hành Động"),
	("phim-co-trang-c14.html", "Cổ Trang"),
];