//! Data models: the JSON envelopes exchanged with the watch/grab/streamc
//! endpoints, plus the bag of metadata parsed out of a detail page.

use alloc::{string::String, vec::Vec};
use komorei::serde::Deserialize;

/// The watch-page XHR reply: `{code:200, html:"<iframe id=playerEmbed …>"}`.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(crate) struct WatchPostResp {
	pub(crate) code: i32,
	pub(crate) html: String,
}

/// One entry of the obfuscated playlist array hidden in the grab page. The
/// network encodes `[{file,label,type,default[,streamUrl,token]}]` as base64
/// and defers `JSON.parse(atob(…))` to the player JS — this source decodes it
/// through the runner's `imports::base64`. A `token` entry means the CDN wants
/// the token header (and the JS rewrites `/key` segment URLs onto `streamUrl`).
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(crate) struct PlaylistEntry {
	pub(crate) file: String,
	pub(crate) label: Option<String>,
	#[serde(rename = "type")]
	pub(crate) kind: Option<String>,
	pub(crate) default: Option<bool>,
	#[serde(rename = "streamUrl")]
	pub(crate) stream_url: Option<String>,
	pub(crate) token: Option<String>,
}

/// The streamc bootstrap response — the `bootstrap` field is the grant token
/// handed to the `issue` POST.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(crate) struct BootstrapResp {
	pub(crate) bootstrap: Option<String>,
	#[serde(rename = "turnstileEnabled")]
	pub(crate) turnstile_enabled: bool,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(crate) struct IssueResp {
	pub(crate) playlist: Option<String>,
	#[serde(rename = "playlistFormat")]
	pub(crate) playlist_format: Option<String>,
}

/// Every parsed metadata slice the source needs from a detail page.
#[derive(Default)]
pub(crate) struct DetailInfo {
	pub(crate) title: String,
	pub(crate) subname: String,
	pub(crate) cover: Option<String>,
	pub(crate) description: String,
	pub(crate) score: Option<f32>,
	pub(crate) directors: Vec<String>,
	pub(crate) genres: Vec<String>,
	pub(crate) countries: Vec<String>,
	pub(crate) year: Option<String>,
	pub(crate) current_episode: Option<i32>,
	pub(crate) total_episodes: Option<i32>,
}