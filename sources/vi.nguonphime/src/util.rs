//! Pure string / number / URL helpers shared by the parsers and the network
//! layer. Nothing here touches the komorei model types or the network.

use alloc::{
	format,
	string::{String, ToString},
	vec::Vec,
};

/// First run of decimal digits ("Tập 24" → "24").
pub(crate) fn parse_episode_number(name: &str) -> String {
	let mut digits = String::new();
	for c in name.chars() {
		if c.is_ascii_digit() {
			digits.push(c);
		} else if !digits.is_empty() {
			break;
		}
	}
	if digits.is_empty() {
		String::from("1")
	} else {
		digits
	}
}

/// First run of decimal digits as an integer (""/"1970xyz" → None).
pub(crate) fn parse_first_int(input: Option<&str>) -> Option<i32> {
	let input = input?;
	let digits: String = input
		.chars()
		.skip_while(|c| !c.is_ascii_digit())
		.take_while(|c| c.is_ascii_digit())
		.collect();
	digits.parse().ok()
}

/// First decimal number (digits + `.`/`,` separators) after the first `:`.
pub(crate) fn parse_first_f32(input: &str) -> Option<f32> {
	let idx = input.find(':')? + 1;
	let v: String = input[idx..]
		.trim_start()
		.chars()
		.take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
		.collect();
	v.replace(',', ".").parse().ok()
}

/// The first two integers in a string ("24 / 47 Tập" → (Some(24), Some(47))).
pub(crate) fn parse_two_ints(input: &str) -> (Option<i32>, Option<i32>) {
	let nums: Vec<i32> = input
		.split(|c: char| !c.is_ascii_digit())
		.filter(|s| !s.is_empty())
		.filter_map(|s| s.parse().ok())
		.collect();
	(nums.first().copied(), nums.get(1).copied())
}

/// Parse `?page=N` (also `&page=N`) out of a pager href.
pub(crate) fn page_param(href: &str) -> Option<i32> {
	let idx = href.find("page=")?;
	let digits: String = href[idx + 5..]
		.chars()
		.take_while(|c| c.is_ascii_digit())
		.collect();
	digits.parse().ok()
}

/// Film id from `Anime.key = "{slug}-f{id}"` (the trailing `-f\d+` part).
pub(crate) fn film_id(key: &str) -> Option<String> {
	let idx = key.rfind("-f")?;
	let digits: String = key[idx + 2..]
		.chars()
		.take_while(|c| c.is_ascii_digit())
		.collect();
	if digits.is_empty() {
		None
	} else {
		Some(digits)
	}
}

/// `"lan-huong-nhu-co-f83892-24-e1007951.html"` → `"lan-huong-nhu-co-f83892"`
/// (the deep-link / watch-path anime key).
pub(crate) fn film_key_prefix(segment: &str) -> Option<String> {
	let seg = segment.trim_end_matches(".html");
	let idx = seg.rfind("-f")?;
	let digits: String = seg[idx + 2..]
		.chars()
		.take_while(|c| c.is_ascii_digit())
		.collect();
	if digits.is_empty() {
		return None;
	}
	Some(seg[..idx + 2 + digits.len()].to_string())
}

/// Scheme+host origin; `Referer`/`Origin` for the streamc grant and media.
pub(crate) fn embed_origin(url: &str) -> Option<String> {
	let (scheme, rest) = if let Some(rest) = url.strip_prefix("https://") {
		("https://", rest)
	} else if let Some(rest) = url.strip_prefix("http://") {
		("http://", rest)
	} else {
		return None;
	};
	let host = rest.split(['/', '?', '#']).next().filter(|h| !h.is_empty())?;
	Some(format!("{scheme}{host}"))
}

/// Absolute cover URLs (the nps3 CDN) are already absolute; just drop empties.
pub(crate) fn absolutize(url: String) -> Option<String> {
	if url.is_empty() {
		None
	} else {
		Some(url)
	}
}

/// The NP Checker body marker — the interstitial page a cookie-less request
/// is bounced through (`<title>NP Checker</title>` + a JS forward + a
/// "Chào mừng…" greeting).
pub(crate) fn is_checker_page(body: &str) -> bool {
	body.contains("NP Checker") || body.contains("Chào mừng bạn đến với chúng tôi")
}

/// Extract the server-switch POST path from the grab page:
/// `var url = '/xem-phim/…?key=…&tim=…&fromEmbed=1&api=nguonphime.site';`.
/// The player also declares `var url = ''` early in the page, so scan for the
/// declaration that actually carries `fromEmbed=1`.
pub(crate) fn from_embed_url(html: &str) -> Option<String> {
	let marker = "var url = '";
	let mut search_from = 0;
	while let Some(rel) = html[search_from..].find(marker) {
		let idx = search_from + rel + marker.len();
		let rest = &html[idx..];
		let end = rest.find('\'').unwrap_or(rest.len());
		let url = &rest[..end];
		if !url.is_empty() && url.contains("fromEmbed=1") {
			return Some(url.to_string());
		}
		search_from = idx + 1;
	}
	None
}

/// `key`/`tim`/any single query param out of a path (for the fromEmbed POST).
pub(crate) fn query_param(path: &str, name: &str) -> Option<String> {
	let idx = path.find(name)?;
	let after = &path[idx + name.len()..];
	let after = after.strip_prefix('=')?;
	let value: String = after
		.chars()
		.take_while(|c| *c != '&')
		.collect();
	if value.is_empty() {
		None
	} else {
		Some(value)
	}
}

/// `base` + the `/…` path a card or episode linked to.
pub(crate) fn link_base(base: &str, path: &str) -> String {
	format!("{base}/{path}")
}