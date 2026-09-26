//! The HTTP layer: page fetches, the player handshake, and the playlist
//! resolution that drives [`crate::crypto`].
//!
//! ## Cloudflare
//!
//! The site sits behind a Cloudflare managed challenge, so a plain request can
//! come back 403 with a `<title></title>` interstitial. That body is one of the
//! markers the app's `KrxHostImpl` looks for, so the host transparently retries
//! the request through a WebView and feeds the resulting cookies back in — this
//! module only has to fail the call rather than cache the interstitial.

extern crate alloc;

use alloc::{
	format,
	string::{String, ToString},
	vec::Vec,
};

use komorei::{
	Result,
	imports::{base64, html::Document, net::Request},
	prelude::println,
};

use crate::{
	crypto::{
		self, PlayerVars, decrypt_segment_url, descramble_content, extract_jti_odd, fn_crypto,
		parse_envelope, parse_player_vars,
	},
	parsers,
};

/// A browser-ish `Accept` for the HTML pages. The host already installs a
/// desktop User-Agent on its shared client, so it is not repeated here.
const ACCEPT_HTML: &str =
	"text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8";
/// The form encoding the site's own XHRs use.
const ACCEPT: &str = "application/x-www-form-urlencoded; charset=UTF-8";

/// How long a single request may take. The site is slow behind the challenge, so
/// this is generous; the host applies its own limits on top.
const TIMEOUT_SECONDS: f64 = 30.0;

/// A playlist ready for the media engine.
pub struct ResolvedStream {
	/// The m3u8 text, with every segment url already decrypted and absolute.
	pub playlist: String,
}

/// Fetch a page and parse it.
pub fn fetch_html(url: &str) -> Result<Document> {
	let response = Request::get(url)
		.map_err(|_| komorei::error!("Không tạo được yêu cầu tới {url}"))?
		.header("Accept", ACCEPT_HTML)
		.header("Accept-Language", "vi,vi-VN;q=0.9,en-US;q=0.9,en;q=0.8")
		.timeout(TIMEOUT_SECONDS)
		.send()
		.map_err(|_| komorei::error!("Không kết nối được tới {url}"))?;

	let status = response.status_code();
	if !(200..300).contains(&status) {
		return Err(komorei::error!("Trang {url} trả về HTTP {status}"));
	}
	response
		.get_html()
		.map_err(|_| komorei::error!("Không đọc được HTML của {url}"))
}

/// Fetch a page and read it as text, without parsing it as HTML.
pub fn fetch_text(url: &str, referer: &str) -> Result<String> {
	let response = Request::get(url)
		.map_err(|_| komorei::error!("Không tạo được yêu cầu tới {url}"))?
		.header("Accept", ACCEPT_HTML)
		.header("Referer", referer)
		.timeout(TIMEOUT_SECONDS)
		.send()
		.map_err(|_| komorei::error!("Không kết nối được tới {url}"))?;

	let status = response.status_code();
	if !(200..300).contains(&status) {
		return Err(komorei::error!("Trang {url} trả về HTTP {status}"));
	}
	response
		.get_string()
		.map_err(|_| komorei::error!("Không đọc được nội dung của {url}"))
}

/// Ask `/ajax/player` for a playable url for one episode.
///
/// The site wants the episode's `data-id` *and* its `data-hash` — the hash is a
/// per-episode signature, so the pair cannot be forged from the id alone. The
/// reply is `{"success":1,"link":…,"playTech":"iframe"}`, where `link` is a
/// single url or a list of them.
pub fn resolve_player_iframe(base: &str, episode_id: &str, hash: &str) -> Result<String> {
	let url = format!("{base}/ajax/player");
	let body = format!("id={}&link={}", percent(episode_id), percent(hash));

	let response = Request::post(url.as_str())
		.map_err(|_| komorei::error!("Không tạo được yêu cầu tới /ajax/player"))?
		.header("Content-Type", ACCEPT)
		.header("Accept", ACCEPT_HTML)
		.header("X-Requested-With", "XMLHttpRequest")
		.header("Origin", base)
		.header("Referer", url.as_str())
		.body(body.as_bytes())
		.timeout(TIMEOUT_SECONDS)
		.send()
		.map_err(|_| komorei::error!("Không kết nối được tới /ajax/player"))?;

	let status = response.status_code();
	if !(200..300).contains(&status) {
		return Err(komorei::error!("/ajax/player trả về HTTP {status}"));
	}
	let text = response
		.get_string()
		.map_err(|_| komorei::error!("Không đọc được phản hồi từ /ajax/player"))?;

	let json: serde_json::Value = serde_json::from_str(&text)
		.map_err(|_| komorei::error!("/ajax/player trả về dữ liệu không hợp lệ"))?;

	// `link` is a bare string for a single server, an array when there are more.
	let link = match json.get("link") {
		Some(serde_json::Value::String(s)) => s.clone(),
		Some(serde_json::Value::Array(items)) => items
			.iter()
			.find_map(|item| item.as_str().map(|s| s.to_string()))
			.unwrap_or_default(),
		_ => String::new(),
	};
	if link.is_empty() {
		return Err(komorei::error!("Tập này chưa có máy phát nào ({text})"));
	}
	Ok(link)
}

/// Walk the whole playback chain and hand back a playable playlist.
///
/// 1. fetch the player page and read its id, token and crypto flags;
/// 2. ask for the playlist, authenticating with the token;
/// 3. unwrap the `X-Envelope` container and decrypt the body;
/// 4. swap every `/hls/…` placeholder for its real segment url.
///
/// `player_url` is the iframe url `/ajax/player` handed back; its host also
/// serves the playlist.
pub fn resolve_stream(player_url: &str) -> Result<ResolvedStream> {
	let player_html = fetch_text(player_url, &format!("{}/", host_of(player_url)))?;
	let vars = parse_player_vars(&player_html)
		.map_err(|why| komorei::error!("Không đọc được trang phát: {why}"))?;
	// The token's `jti`, halved, is the key the segment url cipher runs on.
	let session_key = extract_jti_odd(&vars.avs_sk);

	let playlist = fetch_playlist(player_url, &vars)?;
	Ok(ResolvedStream {
		playlist: decrypt_placeholders(&playlist, &session_key),
	})
}

/// Fetch and decrypt the playlist body for a player page.
fn fetch_playlist(player_url: &str, vars: &PlayerVars) -> Result<String> {
	let host = host_of(player_url);
	// The playlist lives on the same host as the player iframe, and asks for
	// the token twice over: as `token`, and — the site's own quirk — as a
	// base64("cross-origin") `fc` parameter.
	let fc = base64::encode(b"cross-origin");
	let url = format!(
		"https://{host}/playlist/{}/playlist.m3u8?token={}&fc={fc}",
		vars.id, vars.avs_sk
	);

	let response = Request::get(url.as_str())
		.map_err(|_| komorei::error!("Không tạo được yêu cầu playlist"))?
		// The site checks the referer against the playlist url itself.
		.header("Referer", url.as_str())
		.header("Accept", "*/*")
		.timeout(TIMEOUT_SECONDS)
		.send()
		.map_err(|_| komorei::error!("Không tải được playlist"))?;

	let status = response.status_code();
	if !(200..300).contains(&status) {
		return Err(komorei::error!("Playlist trả về HTTP {status}"));
	}

	// The key material rides in a header; the individual `X-edge-Tag` /
	// `X-Cache-Node` / `X-Request-Trace` / `X-Proxy-Digest` headers are the
	// pre-envelope form of the same data and are still worth honouring.
	let envelope = response
		.get_header("X-Envelope")
		.or_else(|| response.get_header("x-envelope"));
	let env = match envelope {
		Some(token) => {
			let env = parse_envelope(&token)
				.map_err(|why| komorei::error!("Envelope không hợp lệ: {why}"))?;
			Some(env)
		}
		None => None,
	};

	let body = response
		.get_string()
		.map_err(|_| komorei::error!("Không đọc được nội dung playlist"))?;

	match env {
		Some(env) => {
			let out = decrypt_playlist_body(&body, &env, vars.harden)?;
			println!(
				"[avs] playlist {} bytes, envelope stag={} etag={} id={} custom={} -> {} bytes; \
				 decoy /chunks/ left={}, real /hls/ found={}",
				body.len(),
				env.stag,
				env.etag,
				env.id,
				env.custom,
				out.len(),
				out.matches("/chunks/").count(),
				out.matches("/hls/").count(),
			);
			Ok(out)
		}
		// Without the envelope there is no key, so the body can only be usable
		// if the site served a plain manifest.
		None => {
			println!(
				"[avs] playlist has NO X-Envelope header, {} bytes",
				body.len()
			);
			Ok(body)
		}
	}
}

/// Unwrap the decoy playlist body into the real manifest.
///
/// The m3u8 tags travel in the clear; the segment lines do not. Every segment
/// line is a decoy `/chunks/…` url whose `_t` parameter is ciphertext, and the
/// concatenation of those blobs is the encrypted real playlist. `_c` is the
/// segment count, not a cipher flag — but its presence is what distinguishes
/// this layout from a plain manifest.
pub(crate) fn decrypt_playlist_body(
	body: &str,
	env: &crypto::Envelope,
	harden: bool,
) -> Result<String> {
	let lines: Vec<&str> = body.split('\n').collect();

	let encrypted = lines
		.iter()
		.any(|line| !line.trim().is_empty() && !line.starts_with('#') && has_c_param(line));
	if !encrypted {
		// Already a plain manifest — nothing to unwrap.
		println!("[avs] body is not encrypted (no _c), passing through");
		return Ok(body.to_string());
	}
	// The envelope's `cn`/`sk` are required; without them the body stays as-is.
	if env.stag.is_empty() || env.etag.is_empty() {
		println!(
			"[avs] body has _c but the envelope is empty (stag={:?} etag={:?}), passing through",
			env.stag, env.etag
		);
		return Ok(body.to_string());
	}

	// Keep the tags the site does not encrypt, and drop the ones it re-emits:
	// `#EXTINF` is rebuilt from the decrypted body, and `#EXT-X-KEY` is
	// superseded by the decrypted `#EXT-X-AVS-SK` session key.
	let mut header: Vec<&str> = Vec::new();
	let mut payload = String::new();
	for line in &lines {
		let is_tag = line.starts_with('#') || line.trim().is_empty();
		if is_tag {
			if !line.starts_with("#EXTINF:")
				&& !line.starts_with("#EXT-X-ENDLIST")
				&& !line.starts_with("#EXT-X-KEY")
				&& !line.trim().is_empty()
			{
				header.push(line);
			}
		} else if let Some(blob) = t_param(line) {
			payload.push_str(blob);
		}
	}
	if payload.is_empty() {
		println!("[avs] no _t parameters found, passing through");
		return Ok(body.to_string());
	}

	let descrambled = descramble_content(&payload, &env.etag);
	let decrypted = fn_crypto(
		&descrambled,
		&env.stag,
		&env.etag,
		&env.custom,
		&env.id,
		harden,
	)
	.map_err(|why| komorei::error!("Không giải được playlist: {why}"))?;

	let mut out = String::new();
	for line in header {
		out.push_str(line);
		out.push('\n');
	}
	out.push('\n');
	out.push_str(&decrypted);
	Ok(out)
}

/// Rewrite every `/hls/…` placeholder into the real segment url, and make any
/// relative url absolute.
fn decrypt_placeholders(playlist: &str, session_key: &str) -> String {
	let origin = String::new();
	let mut out = String::with_capacity(playlist.len());
	for line in playlist.split('\n') {
		if line.starts_with('#') || line.trim().is_empty() {
			out.push_str(line);
			out.push('\n');
			continue;
		}
		let absolute = parsers::absolute(line);
		let decrypted = decrypt_segment_url(&absolute, session_key);
		let _ = &origin;
		out.push_str(&decrypted);
		out.push('\n');
	}
	// The split above appends a trailing newline the input may not have had.
	while out.ends_with("\n\n") {
		out.pop();
	}
	out
}

/// Whether a segment line carries the `_c` parameter.
pub(crate) fn has_c_param(line: &str) -> bool {
	query_value(line, "_c").is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
}

/// The `_t` payload of a decoy segment line.
pub(crate) fn t_param(line: &str) -> Option<&str> {
	query_value(line, "_t").filter(|v| !v.is_empty())
}

/// The value of `key` in a url's query string.
///
/// The predicate has to live *inside* `find_map`: a decoy segment line carries
/// `si`, `seq`, `token`, `_t` and `_c` in that order, so a
/// `find_map(split_once)` followed by a `filter` would stop at `si` and report
/// every later parameter as missing.
pub(crate) fn query_value<'a>(url: &'a str, key: &str) -> Option<&'a str> {
	let query = url.split_once('?')?.1;
	query.split('&').find_map(|pair| {
		let (name, value) = pair.split_once('=')?;
		(name == key).then_some(value)
	})
}

/// The host of an absolute url.
pub fn host_of(url: &str) -> &str {
	let rest = match url.find("://") {
		Some(i) => &url[i + 3..],
		None => return url,
	};
	let end = rest.find('/').unwrap_or(rest.len());
	&rest[..end]
}

/// Percent-encode a form value.
fn percent(value: &str) -> String {
	let mut out = String::with_capacity(value.len());
	for byte in value.as_bytes() {
		match byte {
			b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
				out.push(*byte as char)
			}
			b' ' => out.push('+'),
			other => out.push_str(&format!("%{other:02X}")),
		}
	}
	out
}
