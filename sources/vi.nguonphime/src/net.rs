//! Network layer: the NP Checker bounce handling, the watch-page XHR POSTs,
//! and the two playback-resolver chains (PAI direct HLS / NGC streamc grant).

use alloc::{
	format,
	string::{String, ToString},
};
use komorei::{
	HashMap, Result,
	imports::html::{Document, Html},
	imports::net::{Request, Response},
	imports::std::current_date,
	prelude::*,
};

use crate::catalog::PLAYLIST_FORMAT;
use crate::models::{BootstrapResp, IssueResp, WatchPostResp};
use crate::parsers::extract_playlist;
use crate::util::{embed_origin, from_embed_url, is_checker_page, query_param};

/// Parse an HTML body, transparently surviving the NP Checker bounce (see the
/// crate docs): detect the checker body and re-request once — the session
/// cookies were stored by the app's cookie jar on the first hop.
pub(crate) fn fetch_html(url: &str) -> Result<Document> {
	let parse = |body: String| -> Result<Document> {
		Html::parse(body).map_err(|_| error!("Máy chủ trả về HTML không hợp lệ."))
	};

	let body = Request::get(url)?.string()?;
	if !is_checker_page(&body) {
		return parse(body);
	}
	// Bounced → session cookies are now in the jar; retry the real page once.
	let body = Request::get(url)?.string()?;
	if is_checker_page(&body) {
		bail!("Máy chủ nguồn từ chối (NP Checker): {url}");
	}
	parse(body)
}

/// Raw body (used for grab pages that only carry JS-encoded playlists).
pub(crate) fn fetch_body(url: &str) -> Result<String> {
	Request::get(url)?.string()
}

/// Form-encoded XHR POST (the watch-page player request + the fromEmbed
/// server switch). A cookie-less first POST is bounced through the NP Checker
/// too — its HTML body cannot decode as JSON. In that case the session is
/// warmed with a base GET (which bounces once, storing the cookies) and the
/// POST is retried once, mirroring how a real browser session starts.
pub(crate) fn watch_post(url: &str, body: &str, base: &str, referer: &str) -> Result<WatchPostResp> {
	let send = || -> Result<Response> {
		let request = Request::post(url)
			.map_err(|_| error!("Liên kết nguồn phát không hợp lệ."))?
			.header("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8")
			.header("X-Requested-With", "XMLHttpRequest")
			.header("Origin", base)
			.header("Referer", referer)
			.body(body);
		let resp = request
			.send()
			.map_err(|_| error!("Không kết nối được máy chủ nguồn."))?;
		let status = resp.status_code();
		if status < 200 || status >= 300 {
			bail!("Máy chủ nguồn phát lỗi {status}");
		}
		Ok(resp)
	};

	let text = send()?
		.get_string()
		.map_err(|_| error!("Máy chủ nguồn trả dữ liệu không hợp lệ."))?;
	if let Ok(parsed) = serde_json::from_str::<WatchPostResp>(&text) {
		return Ok(parsed);
	}
	if is_checker_page(&text) {
		// Bounce the NP Checker open (its first GET stores the session
		// cookies) and retry the POST once with the fresh session.
		let _ = fetch_html(&format!("{base}/"));
		let ok = send()?;
		return ok
			.get_json_owned()
			.map_err(|_| error!("Máy chủ nguồn trả dữ liệu không hợp lệ."));
	}
	bail!("Máy chủ nguồn trả dữ liệu không hợp lệ.")
}

/// One watch-page player POST → the signed grab iframe URL. `time`/`key` are
/// not validated by the server, so the unix clock and the server name are
/// sent; `indexL` selects the playback server for the first resolution.
pub(crate) fn watch_iframe_url(base: &str, watch_url: &str, fid: &str, index_l: u8) -> Result<String> {
	let body = format!(
		"fid={fid}&time={}&key=PAI&loadTime=0&indexL={index_l}&indexSL=0",
		current_date()
	);
	let parsed = watch_post(watch_url, &body, base, watch_url)?;
	if parsed.code != 200 || parsed.html.is_empty() {
		bail!("Máy chủ nguồn chưa sẵn sàng tập này (mã {})", parsed.code);
	}
	let doc = Html::parse(parsed.html).map_err(|_| error!("Trang phát trả về HTML không hợp lệ."))?;
	let src = doc
		.select_first("iframe#playerEmbed")
		.and_then(|f| f.attr("src"))
		.filter(|s| !s.is_empty())
		.map(|s| if s.starts_with("//") { format!("https:{s}") } else { s })
		.ok_or_else(|| error!("Không tìm thấy liên kết phát (iframe)."))?;
	Ok(src)
}

/// Resolve the **PAI** playback server: grab page → base64 playlist → the
/// direct HLS url. A `token`-bearing entry would need the token header + the
/// `/key` segment URL rewrite onto `streamUrl`; none of the verified films use
/// it, but the handling is cheap and mirrors the player's `onXhrOpen`.
pub(crate) fn resolve_pai(iframe_url: &str) -> Result<(String, HashMap<String, String>)> {
	let grab = fetch_body(iframe_url)?;
	let entry = extract_playlist(&grab)
		.and_then(|list| list.into_iter().find(|e| !e.file.is_empty()))
		.ok_or_else(|| error!("Không tìm thấy nguồn phát trên trang embed."))?;

	let mut headers = HashMap::new();
	let mut url = entry.file;
	if let Some(token) = entry.token.filter(|t| !t.is_empty()) {
		headers.insert(String::from("token"), token);
		if let Some(stream_url) = entry.stream_url.filter(|s| !s.is_empty())
			&& url.contains("/key")
		{
			// The player swaps the `/key` segment host for the token CDN.
			if let Some(origin) = embed_origin(&url) {
				url = url.replacen(&origin, &stream_url, 1);
			}
		}
	}
	Ok((url, headers))
}

/// Match the network's own embedded behavior … the NGC server is
/// `embed{N}.streamc.xyz` behind the *site's* fromEmbed POST.
pub(crate) fn resolve_ngc(base: &str, watch_url: &str, fid: &str) -> Result<(String, HashMap<String, String>)> {
	let iframe = watch_iframe_url(base, watch_url, fid, 0)?;
	let grab = fetch_body(&iframe)?;
	let embed_path = from_embed_url(&grab)
		.ok_or_else(|| error!("Trang embed không kích hoạt được máy chủ phụ."))?;
	let tim = query_param(&embed_path, "tim").unwrap_or_else(|| current_date().to_string());
	let body = format!("fid={fid}&time={tim}&key=NGC&loadTime=0&indexL=1&indexSL=0");
	let parsed = watch_post(&format!("{base}{embed_path}"), &body, base, watch_url)?;
	let doc = Html::parse(parsed.html).map_err(|_| error!("Trang phát trả về HTML không hợp lệ."))?;
	let embed = doc
		.select_first("iframe#playerEmbed")
		.and_then(|f| f.attr("src"))
		.filter(|s| !s.is_empty())
		.ok_or_else(|| error!("Máy chủ phụ không trả liên kết phát."))?;

	let origin = embed_origin(&embed).ok_or_else(|| error!("Liên kết phát không hợp lệ: {embed}"))?;
	let playlist = resolve_streamc_playlist(&embed, watch_url, &origin)?;
	let mut headers = HashMap::new();
	headers.insert(String::from("Referer"), format!("{origin}/"));
	Ok((playlist, headers))
}

/// Walk the streamc embed's two-step grant (bootstrap → issue) and return the
/// signed HLS playlist URL. `referrer` is the film page the embed would be
/// loaded from; `origin` becomes the `Origin`/`Referer` on the grant POSTs and
/// the media `Referer` for the segment CDN.
pub(crate) fn resolve_streamc_playlist(embed_url: &str, referrer: &str, origin: &str) -> Result<String> {
	let post_json = |body: &str| -> Result<serde_json::Value> {
		let request = Request::post(embed_url)
			.map_err(|_| error!("Địa chỉ nguồn phát không hợp lệ."))?
			.header("Content-Type", "application/json")
			.header("Origin", origin)
			.header("Referer", origin)
			.body(body);
		let resp = request.send().map_err(|_| error!("Không kết nối được máy chủ nguồn phát."))?;
		let status = resp.status_code();
		if status < 200 || status >= 300 {
			bail!("Máy chủ nguồn phát lỗi {status}");
		}
		resp.get_json_owned::<serde_json::Value>()
			.map_err(|_| error!("Máy chủ nguồn phát trả dữ liệu không hợp lệ."))
	};

	let bootstrap_body = serde_json::json!({
		"action": "bootstrap",
		"referrer": referrer,
		"frame_origins": [origin],
		"request_grant": true,
		"playlist_format": PLAYLIST_FORMAT,
		"pretty_url": true,
		"path_chunks": true,
		"bootstrap_format": "json",
	});
	let bootstrap: BootstrapResp = serde_json::from_value(post_json(&bootstrap_body.to_string())?)
		.map_err(|_| error!("Máy chủ nguồn phát trả dữ liệu không hợp lệ."))?;
	let token = bootstrap
		.bootstrap
		.ok_or_else(|| error!("Không nhận được quyền phát từ máy chủ nguồn."))?;

	let issue_body = serde_json::json!({
		"action": "issue",
		"bootstrap": token,
		"turnstile_response": "",
		"playlist_format": PLAYLIST_FORMAT,
		"pretty_url": true,
		"path_chunks": true,
		"frame_origins": [origin],
	});
	let issue: IssueResp = serde_json::from_value(post_json(&issue_body.to_string())?)
		.map_err(|_| error!("Máy chủ nguồn phát trả dữ liệu không hợp lệ."))?;
	let playlist = issue
		.playlist
		.ok_or_else(|| error!("Không lấy được nguồn phát video."))?;

	if let Some(fmt) = issue.playlist_format.as_deref()
		&& fmt != PLAYLIST_FORMAT
	{
		bail!("Nguồn phát dùng định dạng mã hoá ({fmt}), app chưa hỗ trợ.");
	}
	Ok(playlist)
}