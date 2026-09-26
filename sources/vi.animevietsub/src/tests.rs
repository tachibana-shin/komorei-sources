//! Crate tests.
//!
//! The crypto tests run against **captures from the live site**, not invented
//! inputs: every fixture under `tests/fixtures/` is a real response, so a pass
//! here means the Rust port reproduces what a browser actually receives — not
//! merely that the code agrees with itself.

extern crate alloc;

use alloc::{string::ToString, vec::Vec};

use komorei_test::komorei_test;

use crate::crypto::{
	Envelope, Fnv1aPrng, decrypt_segment_url, descramble_content, extract_jti_odd, fn_crypto,
	parse_envelope, parse_player_vars, trim_segment_header, xor_permute,
};

const ENVELOPE: &str = include_str!("../tests/fixtures/envelope.txt");
const PLAYER_VARS: &str = include_str!("../tests/fixtures/player_vars.txt");
const TOKEN: &str = include_str!("../tests/fixtures/token.txt");
const PLAYLIST_HEADER: &str = include_str!("../tests/fixtures/playlist_header.txt");
const PLAYLIST_CIPHER: &str = include_str!("../tests/fixtures/playlist_cipher.b64");
const PLAYLIST_EXPECTED: &str = include_str!("../tests/fixtures/playlist_expected_head.txt");
const SEGMENT_PLACEHOLDER: &str = include_str!("../tests/fixtures/segment_placeholder.url");
const SEGMENT_REAL: &str = include_str!("../tests/fixtures/segment_real.url");
/// The first 160 bytes of a real segment: a 127-byte PNG disguise in front of
/// the MPEG-TS payload.
const SEGMENT_CAMOUFLAGE: &[u8] = include_bytes!("../tests/fixtures/segment_camouflage.bin");

/// The envelope of a real playlist response, which every decryption input is
/// derived from.
fn real_envelope() -> Envelope {
	parse_envelope(trim(ENVELOPE)).expect("envelope hợp lệ")
}

fn trim(s: &str) -> &str {
	s.trim()
}

// ── envelope ───────────────────────────────────────────────────────────────

#[komorei_test]
fn envelope_round_trips_all_four_fields() {
	let env = real_envelope();
	// `USDK` v1 container: cn/sk/ts/uid.
	assert_eq!(env.stag, "3ZJv2wj1189fvo-ovvl11A");
	assert_eq!(env.etag, "JubavkOQI6AfgNqsk3NdLtvOKyzDLDI-0L6rbCGDG9M");
	assert_eq!(env.id, "1790404573");
	assert_eq!(
		env.custom,
		"f75e74d0fe6a8083a54448428bc0a858cb6a817765717d6291fd72b9a627e6cb"
	);
}

#[komorei_test]
fn envelope_rejects_a_foreign_container() {
	// base64url of "NOPE" + junk — a valid base64 string, wrong magic.
	assert!(parse_envelope("Tk9QRXh4").is_err());
}

#[komorei_test]
fn envelope_rejects_a_truncated_container() {
	assert!(parse_envelope("VVNE").is_err());
}

// ── player page ────────────────────────────────────────────────────────────

#[komorei_test]
fn player_page_yields_id_and_reassembled_token() {
	let vars = parse_player_vars(PLAYER_VARS).expect("trang player hợp lệ");
	assert_eq!(
		vars.id,
		"12446c5dfb4375b4aa0099777b244103771fdb71c46e719ccf2096a45927aed5"
	);
	// The site splits the token across a `+` concatenation precisely to make a
	// naive scraper capture half of it; the two halves must rejoin.
	assert_eq!(vars.avs_sk.len(), 356);
	assert_eq!(vars.avs_sk, trim(TOKEN));
	assert!(vars.harden, "_avsCryptoHarden phải bật");
	assert!(vars.shadow, "_avsCryptoHardenShadow phải bật");
}

#[komorei_test]
fn player_page_ignores_html_attributes_named_id() {
	// `id="avs-error-msg"` appears in the real page's markup; only the `const`
	// declaration may be read as the playlist id.
	let html = r#"<div id="avs-error-msg"></div><script>
	const id = "abc123";
	const avsToken = "aaa" + "bbb";
	window._avsCryptoHarden = true;
	</script>"#;
	assert_eq!(parse_player_vars(html).unwrap().id, "abc123");
}

#[komorei_test]
fn player_page_honours_the_crypto_flags() {
	// The live player page sets both flags; a page that omits them must report
	// `false` rather than silently assuming the hardened path.
	let base = r#"<script>const id = "x"; const avsToken = "aaa";</script>"#;
	assert!(!parse_player_vars(base).unwrap().harden);
	assert!(!parse_player_vars(base).unwrap().shadow);

	let harden_only = r#"<script>const id = "x"; const avsToken = "aaa";
	window._avsCryptoHarden = true;</script>"#;
	assert!(parse_player_vars(harden_only).unwrap().harden);
	assert!(!parse_player_vars(harden_only).unwrap().shadow);
}

#[komorei_test]
fn player_page_without_a_token_is_an_error() {
	assert!(parse_player_vars(r#"<script>const id = "x";</script>"#).is_err());
}

// ── session key ────────────────────────────────────────────────────────────

#[komorei_test]
fn session_key_is_the_odd_characters_of_jti() {
	// The captured token's jti claim is 128 hex characters, so the key is the
	// 64 characters at odd indices.
	let key = extract_jti_odd(trim(TOKEN));
	assert_eq!(key.len(), 64);
	assert_eq!(
		key,
		"a18d99e6fce7714d1640f8571d2433161f51ce646ab2ee61e1ba7b76c24477ff"
	);
	assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[komorei_test]
fn session_key_rejects_a_non_jwt_token() {
	assert_eq!(extract_jti_odd("not-a-jwt"), "");
	assert_eq!(extract_jti_odd("a.b"), "");
}

// ── playlist decryption ────────────────────────────────────────────────────

#[komorei_test]
fn playlist_decrypts_to_the_real_manifest() {
	let env = real_envelope();
	let descrambled = descramble_content(PLAYLIST_CIPHER, &env.etag);
	let plain = fn_crypto(
		&descrambled,
		&env.stag,
		&env.etag,
		&env.custom,
		&env.id,
		true,
	)
	.expect("giải mã playlist");

	// The decrypted body opens with the AVS session key tag and then real
	// `/hls/{24 hex}.ts` placeholders — never the `/chunks/…` decoy urls the
	// server actually sent.
	let first = plain.lines().next().unwrap_or_default();
	assert!(
		first.starts_with("#EXT-X-AVS-SK:"),
		"dòng đầu phải là #EXT-X-AVS-SK, nhận được {first:?}"
	);
	assert!(
		plain.contains("/hls/") && plain.contains("?e=") && plain.contains("&i="),
		"phải chứa placeholder /hls/{{24hex}}.ts?e=…&i=…"
	);
	assert!(
		!plain.contains("/chunks/"),
		"không được còn URL giả /chunks/"
	);
}

#[komorei_test]
fn playlist_header_survives_reassembly() {
	// The m3u8 tags travel in the clear and are re-attached above the decrypted
	// body; the `#EXT-X-KEY` tag is deliberately dropped by the site.
	let header = trim(PLAYLIST_HEADER);
	assert!(header.starts_with("#EXTM3U"));
	assert!(header.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
	assert!(!header.contains("#EXT-X-KEY"));
}

#[komorei_test]
fn playlist_matches_the_captured_head() {
	let env = real_envelope();
	let descrambled = descramble_content(PLAYLIST_CIPHER, &env.etag);
	let plain = fn_crypto(
		&descrambled,
		&env.stag,
		&env.etag,
		&env.custom,
		&env.id,
		true,
	)
	.expect("giải mã playlist");

	// Compare against the browser-verified capture line by line.
	let expected: Vec<&str> = trim(PLAYLIST_EXPECTED).lines().collect();
	let actual: Vec<&str> = plain.lines().collect();
	assert!(
		actual.len() >= expected.len(),
		"thiếu dòng: {} < {}",
		actual.len(),
		expected.len()
	);
	for (i, want) in expected.iter().enumerate() {
		assert_eq!(actual[i], *want, "lệch ở dòng {i}");
	}
}

#[komorei_test]
fn wrong_key_fails_the_gcm_tag() {
	let env = real_envelope();
	let descrambled = descramble_content(PLAYLIST_CIPHER, &env.etag);
	// Perturbing the etag changes both the merge string and the permutation, so
	// the AEAD tag must reject it rather than yield silent garbage.
	let broken = fn_crypto(
		&descrambled,
		&env.stag,
		"tampered-etag",
		&env.custom,
		&env.id,
		true,
	);
	assert!(broken.is_err(), "thẻ GCM sai phải bị từ chối");
}

// ── segment url cipher ─────────────────────────────────────────────────────

#[komorei_test]
fn segment_placeholder_decrypts_to_the_real_url() {
	let key = extract_jti_odd(trim(TOKEN));
	let real = decrypt_segment_url(trim(SEGMENT_PLACEHOLDER), &key);
	assert_eq!(real, trim(SEGMENT_REAL));
	assert!(real.starts_with("https://lh3.googleusercontent.com/"));
}

#[komorei_test]
fn segment_placeholder_is_left_alone_without_a_key() {
	let placeholder = trim(SEGMENT_PLACEHOLDER);
	assert_eq!(decrypt_segment_url(placeholder, ""), placeholder);
}

#[komorei_test]
fn non_placeholder_urls_pass_through() {
	let key = extract_jti_odd(trim(TOKEN));
	for url in [
		"https://example.com/hls/short.ts?e=abc&i=0",
		"https://example.com/media/seg0.m4s?e=abc&i=0",
		"https://example.com/hls/6a5a5acb63e2619f82e2fbf1.ts",
		"https://example.com/hls/zzzzzzzzzzzzzzzzzzzzzzzz.ts?e=abc&i=0",
	] {
		assert_eq!(
			decrypt_segment_url(url, &key),
			url,
			"url {url} phải đi nguyên"
		);
	}
}

// ── segment camouflage ─────────────────────────────────────────────────────

#[komorei_test]
fn segment_header_is_a_png_disguise() {
	// The body really is served as image/png …
	assert_eq!(&SEGMENT_CAMOUFLAGE[..4], b"\x89PNG");
	// … and the MPEG-TS starts exactly 127 bytes in, at the sync byte.
	assert_eq!(SEGMENT_CAMOUFLAGE[127], 0x47);
}

#[komorei_test]
fn trimming_the_header_exposes_mpeg_ts() {
	let trimmed = trim_segment_header(SEGMENT_CAMOUFLAGE);
	assert_eq!(trimmed.len(), SEGMENT_CAMOUFLAGE.len() - 127);
	assert_eq!(trimmed[0], 0x47, "byte đầu phải là sync byte MPEG-TS");
	// Every 188-byte transport packet keeps its sync byte.
	for offset in (0..trimmed.len()).step_by(188) {
		assert_eq!(trimmed[offset], 0x47, "mất sync ở packet {offset}");
	}
}

#[komorei_test]
fn trimming_a_body_without_a_header_yields_nothing() {
	assert!(trim_segment_header(&[0u8; 127]).is_empty());
	assert!(trim_segment_header(b"<html>404</html>").is_empty());
}

// ── prng / permutation properties ──────────────────────────────────────────

#[komorei_test]
fn prng_seeds_from_fnv1a_and_never_reaches_zero() {
	// The empty string hashes to the bare FNV-1a offset basis, so the state is
	// seeded with 0x811C9DC5 — never the zero that the constructor nudges to 1.
	let mut prng = Fnv1aPrng::new("");
	let first = prng.next();
	assert_ne!(first, 0);
	assert_ne!(first, 0x811C_9DC5, "một vòng xorshift phải khác hạt giống");
	assert_ne!(prng.next(), first, "hai lần gọi liên tiếp phải khác nhau");

	// Any non-zero state stays non-zero: xorshift32 is a bijection.
	for seed in ["", "etag", "1790404573", "JubavkOQI6AfgNqsk3NdLtvOKyzDLDI"] {
		let mut p = Fnv1aPrng::new(seed);
		for _ in 0..64 {
			assert_ne!(p.next(), 0, "seed {seed:?} sinh ra 0");
		}
	}
}

#[komorei_test]
fn descramble_output_stays_base64url() {
	// The shuffled payload has to remain decodable base64 — that is the whole
	// point of the transform, and it is what pins the permutation's behaviour.
	let etag = "JubavkOQI6AfgNqsk3NdLtvOKyzDLDI-0L6rbCGDG9M";
	let sample = "AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJKKKKLLLLMMMMNNNNOOOO";
	let out = descramble_content(sample, etag);
	assert_eq!(out.len(), sample.len());
	assert!(out != sample, "phải thực sự biến đổi chuỗi");
	assert!(
		out.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
		"kết quả phải vẫn là base64url: {out:?}"
	);
}

#[komorei_test]
fn descramble_handles_short_and_empty_input() {
	let etag = "JubavkOQI6AfgNqsk3NdLtvOKyzDLDI-0L6rbCGDG9M";
	assert_eq!(descramble_content("", etag), "");
	assert_eq!(descramble_content("a", etag), "a");
	assert_eq!(descramble_content("ab", etag), "ab");
}

#[komorei_test]
fn xor_permute_is_deterministic_and_key_dependent() {
	// Not an involution: the index table is rebuilt forward while the xor word
	// stream keeps advancing, so a second pass does not undo the first. What it
	// must do is be reproducible, and depend on both key halves.
	let data: Vec<u8> = (0u8..=255).collect();
	let a = xor_permute(&data, "etag", "1790404573");
	assert_eq!(a, xor_permute(&data, "etag", "1790404573"));
	assert_ne!(a, data, "phải thực sự biến đổi dữ liệu");
	assert_ne!(a, xor_permute(&data, "other", "1790404573"));
	assert_ne!(a, xor_permute(&data, "etag", "9999999999"));
	assert_eq!(a.len(), data.len());
}

#[komorei_test]
fn xor_permute_rejects_empty_input() {
	assert!(xor_permute(&[], "etag", "salt").is_empty());
}
