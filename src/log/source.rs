//! Reading a log source from a file, transparently decompressing it if its
//! name ends in `.gz`. Never consulted implicitly: a `.gz` file is only
//! ever read because the caller named it explicitly on the command line.

use std::path::Path;

use crate::input_limit::{read_capped_file, read_capped_reader};

/// Bytes of one log file, decoded for the parsers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogText {
    /// The file as text. Invalid UTF-8 bytes are replaced when the caller
    /// asked for a lossy decode.
    pub text: String,
    /// Byte offset of the first byte that was not valid UTF-8.
    /// Those bytes are replaced; the rest of the file is kept.
    pub invalid_utf8_at: Option<usize>,
    /// Lines that contained a NUL byte and were blanked (their line
    /// terminator is kept, so later line numbers do not move).
    pub nul_lines_skipped: usize,
}

/// With `lossy` set, a NUL byte skips only its own line unless NULs make up
/// at least this share of the file: UTF-16 text without a byte-order mark
/// is about half NUL, a stray run of NULs from a power cut is a few percent.
const NUL_SHARE_REFUSED: usize = 4;

/// The one decoder for every text input (crontab, timer unit, syslog,
/// journal). A UTF-16 or UTF-32 byte-order mark or any NUL byte means the
/// file is not 8-bit text; reading it as UTF-8 would turn every line into
/// garbage and report "no jobs" as success, so it is refused. A UTF-8 BOM
/// is stripped. With `lossy` set (logs), invalid UTF-8 is replaced and
/// reported in [`LogText::invalid_utf8_at`], and a line with a NUL byte is
/// blanked and counted in [`LogText::nul_lines_skipped`] (one bad byte must
/// not discard the rest of a log), unless a quarter or more of the file is
/// NUL, which is another encoding; without it, invalid UTF-8 and any NUL
/// are errors.
pub(crate) fn decode_text(bytes: Vec<u8>, lossy: bool) -> Result<LogText, String> {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Err(
            "looks like UTF-16 or UTF-32 (byte-order mark); only UTF-8 is supported".into(),
        );
    }
    if bytes.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) {
        return Err("looks like UTF-32 (byte-order mark); only UTF-8 is supported".into());
    }
    let nuls = bytes.iter().filter(|&&b| b == 0).count();
    if nuls > 0 && (!lossy || nuls * NUL_SHARE_REFUSED >= bytes.len()) {
        let at = bytes.iter().position(|&b| b == 0).unwrap_or(0);
        return Err(format!(
            "contains a NUL byte at byte {at}; not a UTF-8 text file"
        ));
    }
    let (text, invalid_utf8_at) = match String::from_utf8(bytes) {
        Ok(text) => (text, None),
        Err(err) => {
            let at = err.utf8_error().valid_up_to();
            if !lossy {
                return Err(format!("invalid UTF-8 at byte {at}"));
            }
            (
                String::from_utf8_lossy(err.as_bytes()).into_owned(),
                Some(at),
            )
        }
    };
    let text = match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => text,
    };
    let (text, nul_lines_skipped) = if nuls > 0 {
        blank_nul_lines(&text)
    } else {
        (text, 0)
    };
    Ok(LogText {
        text,
        invalid_utf8_at,
        nul_lines_skipped,
    })
}

/// Empty every line that contains a NUL, keeping its terminator.
fn blank_nul_lines(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut skipped = 0;
    for line in text.split_inclusive('\n') {
        if line.contains('\0') {
            skipped += 1;
            if line.ends_with('\n') {
                out.push('\n');
            }
        } else {
            out.push_str(line);
        }
    }
    (out, skipped)
}

/// Read `path` as text, gunzipping it first if its extension is `gz`.
/// Invalid UTF-8 is replaced and reported on [`LogText::invalid_utf8_at`].
/// A file past the input limit is not loaded.
pub fn read_log_source(path: &Path) -> std::io::Result<LogText> {
    let invalid = |msg: String| std::io::Error::new(std::io::ErrorKind::InvalidData, msg);
    let bytes = read_capped_file(path)?;
    if path.extension().and_then(|e| e.to_str()) == Some("gz") {
        // MultiGzDecoder, not GzDecoder: some log rotation appends a new
        // gzip member to an existing file rather than recompressing it,
        // and GzDecoder reads only the first member of such a file.
        // The inflated text has the same byte cap as the file on disk.
        let decoder = flate2::read::MultiGzDecoder::new(&bytes[..]);
        let inflated = read_capped_reader(decoder)?;
        decode_text(inflated, true).map_err(invalid)
    } else {
        decode_text(bytes, true).map_err(invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn plain_file_is_read_as_is() {
        let dir = std::env::temp_dir().join(format!("cron-ledger-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plain.log");
        std::fs::write(&path, "hello\n").unwrap();
        let got = read_log_source(&path).unwrap();
        assert_eq!(got.text, "hello\n");
        assert_eq!(got.invalid_utf8_at, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gz_file_is_decompressed() {
        let dir = std::env::temp_dir().join(format!("cron-ledger-test-gz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rotated.log.gz");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"archived line\n").unwrap();
        let compressed = encoder.finish().unwrap();
        std::fs::write(&path, compressed).unwrap();
        assert_eq!(read_log_source(&path).unwrap().text, "archived line\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gz_file_with_two_concatenated_members_decodes_both() {
        // A plain concatenation of two independent gzip streams is a
        // valid .gz file: some log rotation produces exactly this by
        // appending a freshly gzipped chunk to an existing file.
        let dir =
            std::env::temp_dir().join(format!("cron-ledger-test-gz-multi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rotated.log.gz");

        let mut first = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        first.write_all(b"first member\n").unwrap();
        let mut combined = first.finish().unwrap();

        let mut second = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        second.write_all(b"second member\n").unwrap();
        combined.extend(second.finish().unwrap());

        std::fs::write(&path, combined).unwrap();
        assert_eq!(
            read_log_source(&path).unwrap().text,
            "first member\nsecond member\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_utf8_is_replaced_and_the_rest_of_the_file_is_kept() {
        let dir =
            std::env::temp_dir().join(format!("cron-ledger-test-bad-utf8-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.log");
        let mut bytes = b"before ".to_vec();
        let at = bytes.len();
        bytes.push(0xff);
        bytes.extend(b" after\nnext line\n");
        std::fs::write(&path, &bytes).unwrap();

        let got = read_log_source(&path).unwrap();
        assert_eq!(got.invalid_utf8_at, Some(at));
        assert!(got.text.contains('\u{FFFD}'));
        assert!(got.text.ends_with("next line\n"));
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod nul_tests {
    use super::*;

    #[test]
    fn lossy_nul_blanks_only_its_own_line() {
        let got = decode_text(b"one\ntw\0o\nthree\n".to_vec(), true).unwrap();
        assert_eq!(got.text, "one\n\nthree\n");
        assert_eq!(got.nul_lines_skipped, 1);
    }

    #[test]
    fn lossy_file_that_is_mostly_nul_is_refused() {
        assert!(decode_text(b"a\0b\0c\0".to_vec(), true).is_err());
    }

    #[test]
    fn nul_share_boundary_is_one_in_four_for_a_lossy_log() {
        // One NUL byte in four is refused; one in eight is a skipped line.
        assert!(decode_text(b"abc\0".to_vec(), true).is_err());
        let got = decode_text(b"abcde\0fg".to_vec(), true).unwrap();
        assert_eq!(got.nul_lines_skipped, 1);
    }

    #[test]
    fn strict_nul_is_still_refused() {
        assert!(decode_text(b"one\ntw\0o\n".to_vec(), false).is_err());
    }
}
