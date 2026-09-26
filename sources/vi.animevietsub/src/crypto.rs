//! The "AVS shield" that wraps every AnimeVietsub stream.
//!
//! Three separate layers sit between a player page and playable MPEG-TS, and
//! all of them were verified against the live site before being ported here:
//!
//! 1. **Playlist decryption** ([`resolve_playlist`]). `GET
//!    /playlist/{id}/playlist.m3u8` answers with a *decoy* body: the m3u8 tags
//!    are readable, but every segment line is a
//!    `https://…/chunks/{id}/original/{8}/seg{N}.html?…&_t=<blob>&_c=<count>`
//!    url whose `_t` parameter is ciphertext. The real playlist hides inside
//!    the concatenation of all `_t` blobs. Unwrapping it is three steps:
//!    [`descramble_content`] (an LCG-driven shuffle), an AES-256-GCM open whose
//!    key is HMAC-SHA256 over a merge string, and finally [`xor_permute`] to
//!    undo the byte permutation. The key material itself arrives in the
//!    `X-Envelope` response header, a small binary "USDK" container
//!    ([`parse_envelope`]).
//!
//! 2. **Segment url cipher** ([`decrypt_segment_url`]). The playlist that comes
//!    out of layer 1 still points at *placeholders*,
//!    `https://…/hls/{24 hex}.ts?e=<b64>&i=<n>`, which are a redirect
//!    placeholder rather than media. `e` is AES-256-CTR ciphertext under a key
//!    derived by HMAC-SHA256 from the player token, and its plaintext is the
//!    real segment url.
//!
//! 3. **Segment header camouflage** ([`trim_segment_header`]). The real urls
//!    point at a Google CDN object served as `image/png` whose first 127 bytes
//!    are a small valid PNG. The MPEG-TS payload starts right after, at the
//!    `0x47` sync byte, so the header has to come off before the media engine
//!    sees the data.
//!
//! Layers 1 and 2 happen once, in [`resolve_playlist`] / when the playlist is
//! built. Layer 3 is per-response and is exposed to the engine through the
//! [`SegmentDataInterceptor`](komorei::SegmentDataInterceptor) trait, because a
//! source cannot otherwise post-process response bodies.

extern crate alloc;

use alloc::{
	format,
	string::{String, ToString},
	vec,
	vec::Vec,
};

use aes::cipher::{KeyIvInit, StreamCipher};
use aes_gcm::{
	Aes256Gcm, Nonce,
	aead::{Aead, KeyInit, Payload},
};
use ctr::Ctr32BE;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

use komorei::imports::base64;

/// The magic of the `X-Envelope` binary container: ASCII `USDK`.
const ENVELOPE_MAGIC: [u8; 4] = *b"USDK";
/// The only container version the site emits.
const ENVELOPE_VERSION: u8 = 1;
/// The camouflage PNG header length in front of every real segment.
const SEGMENT_HEADER_LEN: usize = 127;

/// FNV-1a seeded with the usual offset basis, advanced by the xorshift32 that
/// the site's obfuscator uses to build every permutation.
///
/// The Kotlin reference takes the *low byte* of each UTF-16 code unit
/// (`ch.code and 0xFF`), not of the UTF-8 encoding, so the walk goes through
/// `encode_utf16` to stay bit-identical for non-ASCII input.
pub struct Fnv1aPrng {
	state: u32,
}

impl Fnv1aPrng {
	pub fn new(value: &str) -> Self {
		let mut hash: u32 = 0x811C_9DC5;
		for unit in value.encode_utf16() {
			hash ^= (unit & 0xFF) as u32;
			hash = hash.wrapping_mul(16_777_619);
		}
		// A zero state would make every draw a fixed point, so it is nudged to 1.
		Self {
			state: if hash == 0 { 1 } else { hash },
		}
	}

	/// `v8 ^= v8 << 13; v8 ^= v8 >>> 17; v8 ^= v8 << 5` — all three operations
	/// stay in 32 bits, and the shifts are wrapping/logical by construction.
	pub fn next(&mut self) -> u32 {
		let mut v = self.state;
		v ^= v << 13;
		v ^= v >> 17;
		v ^= v << 5;
		self.state = v;
		v
	}
}

/// The payload of the `X-Envelope` header — the four strings every playlist
/// decryption step is keyed on.
///
/// The header names are the site's own: `cn` ("content node") becomes the
/// stag/iv source, `sk` the etag, `ts` a timestamp used as a salt, and `uid`
/// the per-session digest.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct Envelope {
	/// `cn` — base64url; its first 12 bytes are the AES-GCM nonce.
	pub stag: String,
	/// `sk` — base64url; the permutation key and the merge string's third field.
	pub etag: String,
	/// `ts` — the merge string's second field and the xor-permute salt.
	pub id: String,
	/// `uid` — the merge string's first field.
	pub custom: String,
}

#[derive(serde::Deserialize)]
struct EnvelopeJson {
	#[serde(default)]
	cn: String,
	#[serde(default)]
	sk: String,
	#[serde(default)]
	ts: String,
	#[serde(default)]
	uid: String,
}

/// Parse the binary `X-Envelope` container.
///
/// Layout: `USDK` magic, a one-byte version, a big-endian `u16` payload length,
/// then that many bytes of JSON. Everything after the payload is a trailing
/// authenticator that the client never uses.
pub fn parse_envelope(token: &str) -> Result<Envelope, &'static str> {
	let buffer = decode_b64url(token).ok_or("Envelope: base64 không hợp lệ")?;
	if buffer.len() < 11 {
		return Err("Envelope quá ngắn");
	}
	if buffer[0..4] != ENVELOPE_MAGIC {
		return Err("Envelope sai magic");
	}
	if buffer[4] != ENVELOPE_VERSION {
		return Err("Envelope sai phiên bản");
	}
	let len = ((buffer[5] as usize) << 8) | buffer[6] as usize;
	if buffer.len() < 7 + len + 4 {
		return Err("Envelope sai độ dài");
	}
	let payload =
		core::str::from_utf8(&buffer[7..7 + len]).map_err(|_| "Envelope: JSON không hợp lệ")?;
	let json: EnvelopeJson =
		serde_json::from_str(payload).map_err(|_| "Envelope: JSON không hợp lệ")?;
	Ok(Envelope {
		stag: json.cn,
		etag: json.sk,
		id: json.ts,
		custom: json.uid,
	})
}

/// The permuter's linear congruential step.
fn lcg_next(state: u32) -> u32 {
	state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
}

/// Undo the seeded shuffle applied to the concatenated `_t` blobs.
///
/// The seed is the first eight hex characters of the etag; a non-hex prefix
/// simply yields seed 0 (the site commonly sends a base64url etag, so the
/// fallback is the normal case, not the exception). The swap pairs are built
/// front-to-back but applied back-to-front, which is what makes the transform
/// its own inverse.
///
/// The reference implementation shuffles `Char`s. The payload is base64url, so
/// ASCII — byte and `char` indices coincide and the byte version is exact.
pub fn descramble_content(content: &str, etag: &str) -> String {
	let mut bytes = content.as_bytes().to_vec();
	let len = bytes.len();
	if len < 2 {
		return content.to_string();
	}

	let seed_hex = &etag[..core::cmp::min(8, etag.len())];
	let mut v56 = u32::from_str_radix(seed_hex, 16).unwrap_or(0);

	let mut pairs: Vec<(usize, usize)> = Vec::with_capacity(len - 1);
	for index in (1..len).rev() {
		v56 = lcg_next(v56);
		pairs.push((index, (v56 % (index as u32 + 1)) as usize));
	}
	for (a, b) in pairs.into_iter().rev() {
		bytes.swap(a, b);
	}
	// Every shuffled byte came from a base64url body, so this cannot be invalid.
	String::from_utf8(bytes).unwrap_or_else(|_| content.to_string())
}

/// Undo the byte permutation layered on top of the AES-GCM plaintext.
///
/// One PRNG drives both phases: the same stream that shuffled the index table
/// then supplies a little-endian 32-bit xor word, refreshed every four bytes.
pub fn xor_permute(data: &[u8], perm_key: &str, perm_salt: &str) -> Vec<u8> {
	let len = data.len();
	if len == 0 {
		return Vec::new();
	}
	let mut prng = Fnv1aPrng::new(&format!("{perm_key}|{perm_salt}"));

	let mut table: Vec<u32> = (0..len as u32).collect();
	for i in (1..len).rev() {
		let shuf = (prng.next() % (i as u32 + 1)) as usize;
		table.swap(i, shuf);
	}

	let mut out = vec![0u8; len];
	let mut shuff: u32 = 0;
	for (i, byte) in data.iter().enumerate() {
		if i & 3 == 0 {
			shuff = prng.next();
		}
		out[table[i] as usize] = *byte ^ ((shuff >> (8 * (i & 3))) as u8);
	}
	out
}

/// Pull the session key out of the player token's `jti` claim, keeping only the
/// characters at odd indices.
///
/// The claim is 128 hex characters; every other one yields the 64-character key
/// the segment url cipher expects. A token that is not a three-part JWT, or that
/// carries no `jti`, yields an empty string — the caller then skips the url
/// cipher, which is still correct because the placeholders redirect.
pub fn extract_jti_odd(token: &str) -> String {
	let parts: Vec<&str> = token.split('.').collect();
	if parts.len() != 3 {
		return String::new();
	}
	let Some(payload) = decode_b64url(parts[1]) else {
		return String::new();
	};
	let Ok(text) = core::str::from_utf8(&payload) else {
		return String::new();
	};
	let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else {
		return String::new();
	};
	let Some(jti) = json.get("jti").and_then(|v| v.as_str()) else {
		return String::new();
	};
	if jti.is_empty() {
		return String::new();
	}
	jti.chars()
		.enumerate()
		.filter(|(i, _)| i % 2 == 1)
		.map(|(_, ch)| ch)
		.collect()
}

/// Decrypt the concatenated `_t` blobs into the real playlist text.
///
/// With `harden` (the live configuration — the player page sets
/// `_avsCryptoHarden = true`) the merge string carries a fourth, empty field
/// and the plaintext is run through [`xor_permute`].
pub fn fn_crypto(
	content: &str,
	stag: &str,
	etag: &str,
	custom: &str,
	id: &str,
	harden: bool,
) -> Result<String, &'static str> {
	let stag_bytes = decode_b64url(stag).ok_or("stag: base64 không hợp lệ")?;
	if stag_bytes.len() < 12 {
		return Err("stag quá ngắn cho IV 12 byte");
	}

	let merge_key = if harden {
		format!("{custom}:{id}:{etag}:0")
	} else {
		format!("{custom}:{id}:{etag}")
	};

	let mut mac =
		<HmacSha256 as Mac>::new_from_slice(&stag_bytes).map_err(|_| "HMAC: key không hợp lệ")?;
	mac.update(merge_key.as_bytes());
	let secret: [u8; 32] = mac.finalize().into_bytes().into();

	let ciphertext = decode_b64url(content).ok_or("Nội dung: base64 không hợp lệ")?;
	let nonce = Nonce::from_slice(&stag_bytes[..12]);
	let plaintext = Aes256Gcm::new_from_slice(&secret)
		.map_err(|_| "AES-GCM: key không hợp lệ")?
		.decrypt(
			nonce,
			Payload {
				msg: &ciphertext,
				aad: &[],
			},
		)
		.map_err(|_| "AES-GCM: thẻ xác thực sai")?;

	let body = if harden {
		xor_permute(&plaintext, etag, id)
	} else {
		plaintext
	};
	Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Turn one `/hls/{24 hex}.ts?e=…&i=…` placeholder into the real segment url.
///
/// The 24 hex characters are the file id; `e` is the encrypted url and `i` the
/// segment index, which seeds the counter block's last four bytes. Any url that
/// does not match the placeholder shape is returned untouched, so a playlist
/// that already carries direct urls keeps working.
pub fn decrypt_segment_url(url: &str, sk_global: &str) -> String {
	if sk_global.is_empty() {
		return url.to_string();
	}
	let Some((path, query)) = split_url(url) else {
		return url.to_string();
	};
	let Some(file_id) = file_id_of(path) else {
		return url.to_string();
	};

	let params = query_params(query);
	let Some(encrypted) = params.get("e") else {
		return url.to_string();
	};
	let Some(index) = params.get("i").and_then(|v| v.parse::<u32>().ok()) else {
		return url.to_string();
	};
	let Some(ciphertext) = decode_b64url(encrypted) else {
		return url.to_string();
	};

	let mut mac = match <HmacSha256 as Mac>::new_from_slice(sk_global.as_bytes()) {
		Ok(m) => m,
		Err(_) => return url.to_string(),
	};
	mac.update(format!("url-cipher|{file_id}").as_bytes());
	let secret: [u8; 32] = mac.finalize().into_bytes().into();

	let mut counter = [0u8; 16];
	counter[12..16].copy_from_slice(&index.to_be_bytes());
	let mut buffer = ciphertext;
	match Ctr32BE::<aes::Aes256>::new_from_slices(&secret, &counter) {
		Ok(mut cipher) => {
			cipher.apply_keystream(&mut buffer);
		}
		Err(_) => return url.to_string(),
	}
	String::from_utf8_lossy(&buffer).into_owned()
}

/// Strip the camouflage PNG header so the media engine sees the MPEG-TS.
///
/// A body too short to carry a header — an error page, say — becomes empty
/// rather than passing the junk through.
pub fn trim_segment_header(data: &[u8]) -> Vec<u8> {
	if data.len() <= SEGMENT_HEADER_LEN {
		return Vec::new();
	}
	data[SEGMENT_HEADER_LEN..].to_vec()
}

/// Everything the player page hands us, extracted from its inline script.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct PlayerVars {
	/// `const id` — the playlist id in the `/playlist/{id}/…` path.
	pub id: String,
	/// The reassembled `const avsToken` (the site splits it across `+`
	/// concatenations so a naive scraper captures half of it).
	pub avs_sk: String,
	/// `window._avsCryptoHarden === true`.
	pub harden: bool,
	/// `window._avsCryptoHardenShadow === true`.
	pub shadow: bool,
}

/// Pull the playlist id, token and crypto flags out of a player page.
///
/// Every value lives in one inline `<script>`: `const id = "…"`, then
/// `const avsToken = "…" + "…"`, then the `_avs*` assignments. The token is
/// deliberately split, so it is matched as a run of string literals and
/// concatenated rather than read once.
pub fn parse_player_vars(html: &str) -> Result<PlayerVars, &'static str> {
	// `const id` / `const avsToken` — deliberately anchored on the `const`
	// keyword. A bare `id = "…"` search happily matches the `id="avs-error-msg"`
	// attributes scattered through the page's markup instead.
	let id = const_string_literal(html, "id").ok_or("Trang player: thiếu id")?;
	let avs_sk = concat_const_literal(html, "avsToken").ok_or("Trang player: thiếu avsToken")?;
	if avs_sk.is_empty() {
		return Err("Trang player: avsToken rỗng");
	}

	Ok(PlayerVars {
		id: id.to_string(),
		avs_sk,
		harden: flag_is_true(html, "_avsCryptoHarden"),
		shadow: flag_is_true(html, "_avsCryptoHardenShadow"),
	})
}

/// The value of a `const NAME = "literal";` declaration.
fn const_string_literal<'a>(html: &'a str, name: &str) -> Option<&'a str> {
	let body = const_declaration(html, name)?;
	read_string_literal(body.trim_start())
}

/// The joined value of a `const NAME = "a" + "b" …;` declaration.
fn concat_const_literal(html: &str, name: &str) -> Option<String> {
	let body = const_declaration(html, name)?;
	concat_string_literals(body)
}

/// The right-hand side of a `const NAME = …;` declaration, up to the `;`.
///
/// Anchoring on `const` keeps this off the `window.NAME = …` alias the page
/// also assigns, and off any attribute that merely contains the name.
fn const_declaration<'a>(html: &'a str, name: &str) -> Option<&'a str> {
	let mut from = 0usize;
	while let Some(at) = html[from..].find(name) {
		let start = from + at;
		let name_is_token = html[..start]
			.chars()
			.next_back()
			.map(|c| !(c.is_alphanumeric() || c == '_' || c == '$'))
			.unwrap_or(true);
		if name_is_token {
			let head = html[..start].trim_end();
			if let Some(before_const) = head.strip_suffix("const") {
				// `const` itself must be a whole token, not e.g. `myconst`.
				let const_is_token = before_const
					.chars()
					.next_back()
					.map(|c| !(c.is_alphanumeric() || c == '_' || c == '$'))
					.unwrap_or(true);
				if const_is_token
					&& let Some(rest) = html[start + name.len()..].trim_start().strip_prefix('=')
					&& let Some(end) = rest.find(';')
				{
					return Some(&rest[..end]);
				}
			}
		}
		from = start + name.len();
	}
	None
}

/// Join every `"…"` literal in a run of `+` concatenations.
fn concat_string_literals(expr: &str) -> Option<String> {
	let mut out = String::new();
	let mut rest = expr.trim();
	let mut found = false;
	while let Some(value) = read_string_literal(rest) {
		found = true;
		out.push_str(value);
		rest = rest[value.len() + 2..].trim_start();
		if let Some(next) = rest.strip_prefix('+') {
			rest = next.trim_start();
		} else {
			break;
		}
	}
	if found { Some(out) } else { None }
}

/// A single `"…"` or `'…'` literal at the start of `s` (no escapes occur in the
/// values this source reads).
fn read_string_literal(s: &str) -> Option<&str> {
	let bytes = s.as_bytes();
	let quote = match bytes.first()? {
		b'"' => b'"',
		b'\'' => b'\'',
		_ => return None,
	};
	let end = s[1..].find(quote as char)? + 1;
	Some(&s[1..end])
}

/// `NAME = true` — the page writes these bare (`window._avsCryptoHarden = true`),
/// so a string-literal reader would always miss them.
fn flag_is_true(html: &str, name: &str) -> bool {
	let mut from = 0usize;
	while let Some(at) = html[from..].find(name) {
		let start = from + at;
		let name_is_token = html[..start]
			.chars()
			.next_back()
			.map(|c| !(c.is_alphanumeric() || c == '_' || c == '$'))
			.unwrap_or(true);
		if name_is_token {
			let after = html[start + name.len()..].trim_start();
			if let Some(rest) = after.strip_prefix('=')
				&& rest.trim_start().starts_with("true")
			{
				return true;
			}
		}
		from = start + name.len();
	}
	false
}

/// Split a url into (path, query) without pulling in a url parser.
fn split_url(url: &str) -> Option<(&str, &str)> {
	// Skip `scheme://` so the authority cannot be mistaken for the path.
	let after_scheme = url.find("://").map(|i| i + 3).unwrap_or(0);
	let rest = &url[after_scheme..];
	let host_end = rest.find('/')?;
	let path = &rest[host_end..];
	Some(match path.find('?') {
		Some(q) => (&path[..q], &path[q + 1..]),
		None => (path, ""),
	})
}

/// The 24 hex characters of a `/hls/{id}.ts` placeholder.
fn file_id_of(path: &str) -> Option<&str> {
	let rest = path.strip_prefix("/hls/")?;
	let candidate = rest.split('/').next()?;
	let stem = candidate.strip_suffix(".ts")?;
	if stem.len() == 24 && stem.bytes().all(|b| b.is_ascii_hexdigit()) {
		Some(stem)
	} else {
		None
	}
}

/// Parse a query string into a small owned map.
///
/// Owned rather than borrowed: the values are handed to [`decode_b64url`],
/// which has to normalise them into a new string anyway, and borrowing would
/// have meant leaking the percent-decoded copies to keep them alive.
fn query_params(query: &str) -> alloc::collections::BTreeMap<String, String> {
	let mut map = alloc::collections::BTreeMap::new();
	for pair in query.split('&').filter(|p| !p.is_empty()) {
		let (key, value) = match pair.split_once('=') {
			Some((k, v)) => (k, v),
			None => (pair, ""),
		};
		map.insert(percent_decode(key), percent_decode(value));
	}
	map
}

/// Minimal `%XX` decoding — these values are base64url plus `-._~`, never a
/// literal `+`, so `+` is left alone rather than folded to a space.
fn percent_decode(input: &str) -> String {
	if !input.contains('%') {
		return input.to_string();
	}
	let bytes = input.as_bytes();
	let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
	let mut i = 0;
	while i < bytes.len() {
		if bytes[i] == b'%' && i + 2 < bytes.len() {
			let hi = (bytes[i + 1] as char).to_digit(16);
			let lo = (bytes[i + 2] as char).to_digit(16);
			if let (Some(hi), Some(lo)) = (hi, lo) {
				out.push(((hi << 4) | lo) as u8);
				i += 3;
				continue;
			}
		}
		out.push(bytes[i]);
		i += 1;
	}
	String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

/// base64url → bytes. The runner's native `base64` import already tolerates
/// both alphabets, padded or not, so this only has to normalise `-`/`_` and
/// restore the padding a bare base64 decoder insists on.
fn decode_b64url(input: &str) -> Option<Vec<u8>> {
	let mut normalised = String::with_capacity(input.len() + 3);
	for ch in input.chars() {
		match ch {
			'-' => normalised.push('+'),
			'_' => normalised.push('/'),
			other => normalised.push(other),
		}
	}
	while !normalised.len().is_multiple_of(4) {
		normalised.push('=');
	}
	base64::decode(normalised.as_str())
}
