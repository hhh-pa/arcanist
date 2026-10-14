//! `composer` linter: stale lock files (T1128).
//!
//! PHP reference: `src/lint/linter/ArcanistComposerLinter.php`.
//!
//! Scope: when the linted file is `composer.json`, hash its bytes with MD5
//! and compare against the `hash` field of the sibling `composer.lock`,
//! raising `COMPOSER1` on mismatch. `composer.lock` itself and every other
//! file are skipped, mirroring the PHP `switch` on the basename.
//!
//! Out of scope / divergences: a missing/unreadable lock file or an
//! invalid lock document stays silent (PHP throws, aborting the run; the
//! fallible [`check_pair`] surfaces invalid documents to direct callers).
//! Findings use line/column 1 (PHP raises path-level messages with no
//! line). Note the check itself mirrors PHP literally (MD5 of the raw
//! `composer.json` bytes versus `hash`); real Composer uses a different
//! content hash, so modern lock files may legitimately mismatch.

extern crate self as arc_rs;

use arc_rs::lint::{LintMessage, Linter, SEVERITY_ERROR};

/// Full code for stale lock files (`COMPOSER` + `LINT_OUT_OF_DATE`).
pub const CODE_OUT_OF_DATE: &str = "COMPOSER1";
/// Display name for stale lock files.
pub const NAME_OUT_OF_DATE: &str = "Lock file out-of-date";

/// Composer lock-file freshness linter.
#[derive(Debug, Default)]
pub struct ComposerLinter;

impl ComposerLinter {
    /// New composer linter.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Linter for ComposerLinter {
    fn name(&self) -> &'static str {
        "COMPOSER"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !is_composer_json(path) || data.contains(&0) {
            return Vec::new();
        }
        let lock_path = std::path::Path::new(path).with_file_name("composer.lock");
        let lock_text = match std::fs::read_to_string(&lock_path) {
            Ok(text) => text,
            Err(_) => return Vec::new(),
        };
        let stale = match check_pair(data, &lock_text) {
            Ok(stale) => stale,
            Err(_) => return Vec::new(),
        };
        if !stale {
            return Vec::new();
        }
        vec![LintMessage {
            path: path.to_string(),
            line: 1,
            col: 1,
            code: CODE_OUT_OF_DATE.to_string(),
            severity: SEVERITY_ERROR.to_string(),
            name: NAME_OUT_OF_DATE.to_string(),
            description: "The 'composer.lock' file seems to be out-of-date. \
                          You probably need to run `composer update`."
                .to_string(),
        }]
    }
}

/// True when `path` names a `composer.json` file (case-sensitive basename,
/// mirroring the PHP `switch`).
#[must_use]
pub fn is_composer_json(path: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .is_some_and(|name| name == "composer.json")
}

/// True when the lock text disagrees with the `composer.json` bytes.
///
/// Returns `Err` for invalid lock documents (PHP throws); a missing or
/// non-string `hash` counts as stale (PHP compares against null, which
/// never equals the MD5 hex).
pub fn check_pair(composer_json: &[u8], lock_text: &str) -> Result<bool, String> {
    let hash = lock_hash(lock_text)?;
    Ok(hash.as_deref() != Some(md5_hex(composer_json).as_str()))
}

/// Extract the `hash` field from a `composer.lock` document.
pub fn lock_hash(lock_text: &str) -> Result<Option<String>, String> {
    let value: serde_json::Value = serde_json::from_str(lock_text)
        .map_err(|error| format!("invalid composer.lock: {error}"))?;
    match value.as_object() {
        Some(object) => Ok(object
            .get("hash")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)),
        None => {
            if value.is_array() {
                Ok(None)
            } else {
                Err("invalid composer.lock: expected a JSON object".to_string())
            }
        }
    }
}

/// Lowercase MD5 hex of `data` (RFC 1321, dependency-free).
#[must_use]
pub fn md5_hex(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = md5_digest(data);
    let mut text = String::with_capacity(32);
    for byte in digest {
        text.push(HEX[usize::from(byte >> 4)] as char);
        text.push(HEX[usize::from(byte & 15)] as char);
    }
    text
}

fn md5_digest(data: &[u8]) -> [u8; 16] {
    let mut state = [0x6745_2301u32, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    let bit_len = (data.len() as u64).wrapping_mul(8);

    let mut input = data.to_vec();
    input.push(0x80);
    while input.len() % 64 != 56 {
        input.push(0);
    }
    input.extend_from_slice(&bit_len.to_le_bytes());

    for block in input.as_chunks::<64>().0 {
        let mut words = [0u32; 16];
        for (index, word) in words.iter_mut().enumerate() {
            let start = index * 4;
            *word = u32::from_le_bytes([
                block[start],
                block[start + 1],
                block[start + 2],
                block[start + 3],
            ]);
        }
        md5_round(&mut state, &words);
    }

    let mut digest = [0u8; 16];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    digest
}

#[rustfmt::skip]
const MD5_TABLE: [u32; 64] = [
    0xd76a_a478, 0xe8c7_b756, 0x2420_70db, 0xc1bd_ceee,
    0xf57c_0faf, 0x4787_c62a, 0xa830_4613, 0xfd46_9501,
    0x6980_98d8, 0x8b44_f7af, 0xffff_5bb1, 0x895c_d7be,
    0x6b90_1122, 0xfd98_7193, 0xa679_438e, 0x49b4_0821,
    0xf61e_2562, 0xc040_b340, 0x265e_5a51, 0xe9b6_c7aa,
    0xd62f_105d, 0x0244_1453, 0xd8a1_e681, 0xe7d3_fbc8,
    0x21e1_cde6, 0xc337_07d6, 0xf4d5_0d87, 0x455a_14ed,
    0xa9e3_e905, 0xfcef_a3f8, 0x676f_02d9, 0x8d2a_4c8a,
    0xfffa_3942, 0x8771_f681, 0x6d9d_6122, 0xfde5_380c,
    0xa4be_ea44, 0x4bde_cfa9, 0xf6bb_4b60, 0xbebf_bc70,
    0x289b_7ec6, 0xeaa1_27fa, 0xd4ef_3085, 0x0488_1d05,
    0xd9d4_d039, 0xe6db_99e5, 0x1fa2_7cf8, 0xc4ac_5665,
    0xf429_2244, 0x432a_ff97, 0xab94_23a7, 0xfc93_a039,
    0x655b_59c3, 0x8f0c_cc92, 0xffef_f47d, 0x8584_5dd1,
    0x6fa8_7e4f, 0xfe2c_e6e0, 0xa301_4314, 0x4e08_11a1,
    0xf753_7e82, 0xbd3a_f235, 0x2ad7_d2bb, 0xeb86_d391,
];

fn md5_round(state: &mut [u32; 4], words: &[u32; 16]) {
    let [mut a, mut b, mut c, mut d] = *state;
    for round in 0..64 {
        let (index, shift, choice) = match round {
            0..16 => (round, [7, 12, 17, 22][round % 4], (b & c) | (!b & d)),
            16..32 => (
                (5 * round + 1) % 16,
                [5, 9, 14, 20][round % 4],
                (d & b) | (!d & c),
            ),
            32..48 => ((3 * round + 5) % 16, [4, 11, 16, 23][round % 4], b ^ c ^ d),
            _ => ((7 * round) % 16, [6, 10, 15, 21][round % 4], c ^ (b | !d)),
        };
        let sum = a
            .wrapping_add(choice)
            .wrapping_add(MD5_TABLE[round])
            .wrapping_add(words[index]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(sum.rotate_left(shift));
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

#[cfg(test)]
#[path = "lint_composer_tests.rs"]
mod lint_composer_tests;
