//! Tests for the playlist-body unwrapping and the url/query helpers around it.
//!
//! The crypto tests in `tests.rs` cover the primitives; these cover the glue —
//! the part that decides *whether* a body is the encrypted decoy and pulls the
//! right query parameters off it. That glue is where the one bug this file
//! exists for lived: a `find_map`/`filter` pair that reported every parameter
//! after the first one as missing, which silently handed ExoPlayer the decoy
//! `/chunks/` urls and turned into a 403 on the first segment.

use alloc::{format, string::String, string::ToString, vec::Vec};

use komorei_test::komorei_test;

use crate::{
	crypto::Envelope,
	net::{decrypt_playlist_body, has_c_param, host_of, query_value, t_param},
};

/// One real decoy segment line, trimmed — the query is verbatim, with the JWT
/// and the `_t` blob abbreviated because only their *shape* matters here.
const DECOY: &str = "https://storage.googleapiscdn.com/chunks/6a5a5acb63e2619f82e2fbf1/original/CRty1aLT0JFjnTcv/seg000.html?si=0&seq=0&token=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJhdnMtdXNlciJ9.abc&_t=U_FpT85JEaXdczBepLLjIofhlEct&_c=269";

/// A real, fully unencrypted playlist body as the site serves it: readable m3u8
/// tags, and every segment line a decoy url whose `_t` is the ciphertext.
const DECOY_BODY: &str = concat!(
	"#EXTM3U\n",
	"#EXT-X-VERSION:3\n",
	"#EXT-X-PLAYLIST-TYPE:VOD\n",
	"#EXT-X-TARGETDURATION:10\n",
	"#EXT-X-MEDIA-SEQUENCE:0\n",
	"#EXT-X-KEY:METHOD=SAMPLE-AES-CTR,URI=\"https://storage.googleapiscdn.com/seg-key/abc\",IV=0x00,KEYFORMAT=\"urn:avs:shield:v3\"\n",
	"#EXTINF:3.253244,\n",
	"https://storage.googleapiscdn.com/chunks/aa11/original/xx/seg000.html?si=0&seq=0&token=JWT&_t=BBBB&_c=269\n",
	"#EXTINF:5.296956,\n",
	"https://storage.googleapiscdn.com/chunks/aa11/original/yy/seg001.html?si=1&seq=1&token=JWT&_t=CCCC&_c=269\n",
	"#EXT-X-ENDLIST\n",
);

#[komorei_test]
fn query_value_finds_parameters_past_the_first() {
	// The regression this file guards: `si` comes first, so a helper that stops
	// at the first `key=value` pair reports `_c`, `_t` and `token` as absent.
	assert_eq!(query_value(DECOY, "si"), Some("0"));
	assert_eq!(query_value(DECOY, "seq"), Some("0"));
	assert_eq!(
		query_value(DECOY, "token"),
		Some("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJhdnMtdXNlciJ9.abc")
	);
	assert_eq!(
		query_value(DECOY, "_t"),
		Some("U_FpT85JEaXdczBepLLjIofhlEct")
	);
	assert_eq!(query_value(DECOY, "_c"), Some("269"));
}

#[komorei_test]
fn query_value_reports_absent_parameters() {
	assert_eq!(query_value(DECOY, "e"), None);
	assert_eq!(query_value(DECOY, ""), None);
	// No query string at all.
	assert_eq!(query_value("https://example.com/a/b", "si"), None);
	// A bare `?` with nothing after it.
	assert_eq!(query_value("https://example.com/a?", "si"), None);
}

#[komorei_test]
fn decoy_lines_are_recognised_as_encrypted() {
	assert!(
		has_c_param(DECOY),
		"a decoy line must be recognised as encrypted"
	);
	assert_eq!(t_param(DECOY), Some("U_FpT85JEaXdczBepLLjIofhlEct"));
}

#[komorei_test]
fn non_decoy_lines_are_not_treated_as_encrypted() {
	// A plain segment url has no `_c` at all, and a non-numeric `_c` is some
	// other parameter that happens to share the name.
	assert!(!has_c_param("https://example.com/seg0.ts?token=JWT"));
	assert!(!has_c_param("https://example.com/seg0.ts?_c=abc"));
	assert!(!has_c_param("https://example.com/seg0.ts?_c="));
	assert!(!has_c_param("#EXTINF:3.25,"));
}

#[komorei_test]
fn encrypted_body_is_not_passed_through() {
	// With an empty envelope there is no key, so the body must come back
	// unchanged rather than half-unwrapped — but it must be *recognised*, which
	// is what the "có _c nhưng envelope rỗng" branch reports.
	let env = Envelope::default();
	let out = decrypt_playlist_body(DECOY_BODY, &env, true).expect("must not fail");
	assert_eq!(out, DECOY_BODY, "không có key thì phải trả nguyên body");
	assert!(
		out.contains("/chunks/"),
		"still the decoy url, which is correct without a key"
	);
}

#[komorei_test]
fn plain_manifest_is_passed_through() {
	// No `_c` anywhere: the site served a readable manifest, so there is
	// nothing to unwrap and nothing may be lost.
	let plain = "#EXTM3U\n#EXTINF:3.2,\nhttps://example.com/seg0.ts\n#EXT-X-ENDLIST\n";
	let env = Envelope {
		stag: "AAAA".into(),
		etag: "BBBB".into(),
		id: "1".into(),
		custom: "x".into(),
	};
	let out = decrypt_playlist_body(plain, &env, true).expect("must not fail");
	assert_eq!(out, plain);
}

#[komorei_test]
fn real_ciphertext_unwraps_into_placeholders() {
	// The full path, on the captured envelope: the concatenated `_t` blobs must
	// decrypt to the real playlist of `/hls/…` placeholders, with the decoy
	// `/chunks/` urls and the site's own `#EXT-X-KEY` both gone.
	const ENVELOPE: &str = include_str!("../tests/fixtures/envelope.txt");
	const CIPHER: &str = include_str!("../tests/fixtures/playlist_cipher.b64");
	const HEADER: &str = include_str!("../tests/fixtures/playlist_header.txt");
	const EXPECTED: &str = include_str!("../tests/fixtures/playlist_expected_head.txt");

	let env = crate::crypto::parse_envelope(ENVELOPE.trim()).expect("envelope parses");
	// Rebuild the decoy body the site would have sent: clear tags, then the
	// ciphertext sliced back into per-segment `_t` blobs.
	let blob_len = CIPHER.len() / 4;
	let mut body = String::from(HEADER.trim());
	body.push_str("\n#EXT-X-KEY:METHOD=SAMPLE-AES-CTR,URI=\"https://example/key\",IV=0x0\n");
	for i in 0..4 {
		let from = i * blob_len;
		let to = (from + blob_len).min(CIPHER.len());
		let _ = i;
		body.push_str("#EXTINF:3.25,\n");
		body.push_str(&format!(
			"https://storage.googleapiscdn.com/chunks/aa11/original/xx/seg00{i}.html?si={i}&seq={i}&token=JWT&_t={}&_c=4\n",
			&CIPHER[from..to]
		));
	}
	body.push_str("#EXT-X-ENDLIST\n");

	let out = decrypt_playlist_body(&body, &env, true).expect("decryption must succeed");

	assert!(!out.contains("/chunks/"), "url giả /chunks/ phải biến mất");
	assert!(out.contains("/hls/"), "phải còn placeholder /hls/");
	assert!(
		!out.contains("#EXT-X-KEY"),
		"#EXT-X-KEY của site phải bị loại"
	);
	assert!(out.contains("#EXTM3U"), "tag rõ của site phải được giữ");

	// The decrypted body has to match the browser-verified capture. `out` is
	// the site's clear tags followed by the decrypted body, so the comparison
	// starts at the first decrypted line.
	let start = out
		.lines()
		.position(|l| l.starts_with("#EXT-X-AVS-SK:"))
		.expect("a #EXT-X-AVS-SK line must follow decryption");
	let actual: Vec<&str> = out.lines().skip(start).collect();
	let expected: Vec<&str> = EXPECTED.trim().lines().collect();
	for (i, want) in expected.iter().enumerate() {
		assert_eq!(actual[i], *want, "mismatch on line {i}");
	}
}

#[komorei_test]
fn host_extraction_handles_every_url_shape() {
	assert_eq!(
		host_of("https://storage.googleapiscdn.com/player/abc"),
		"storage.googleapiscdn.com"
	);
	assert_eq!(host_of("https://animevietsub.li/"), "animevietsub.li");
	assert_eq!(host_of("https://animevietsub.li"), "animevietsub.li");
	assert_eq!(host_of("http://a.b.c:8080/x/y?z=1"), "a.b.c:8080");
}
