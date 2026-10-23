//! Reading a log source from a file, transparently decompressing it if its
//! name ends in `.gz`. Never consulted implicitly: a `.gz` file is only
//! ever read because the caller named it explicitly on the command line.

use std::io::Read;
use std::path::Path;

/// Bytes of one log file, decoded for the parsers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogText {
    pub text: String,
    /// Byte offset of the first byte that was not valid UTF-8.
    /// Those bytes are replaced; the rest of the file is kept.
    pub invalid_utf8_at: Option<usize>,
}

/// Decode log bytes. Invalid UTF-8 is replaced and reported. It does not
/// fail the read: one bad byte must not discard the rest of the file.
pub(crate) fn decode_log_bytes(bytes: Vec<u8>) -> LogText {
    match String::from_utf8(bytes) {
        Ok(text) => LogText {
            text,
            invalid_utf8_at: None,
        },
        Err(err) => {
            let invalid_utf8_at = Some(err.utf8_error().valid_up_to());
            LogText {
                text: String::from_utf8_lossy(err.as_bytes()).into_owned(),
                invalid_utf8_at,
            }
        }
    }
}

/// Read `path` as text, gunzipping it first if its extension is `gz`.
/// Invalid UTF-8 is replaced; see [`decode_log_bytes`].
pub fn read_log_source(path: &Path) -> std::io::Result<LogText> {
    let bytes = std::fs::read(path)?;
    if path.extension().and_then(|e| e.to_str()) == Some("gz") {
        // MultiGzDecoder, not GzDecoder: some log rotation appends a new
        // gzip member to an existing file rather than recompressing it,
        // and GzDecoder reads only the first member of such a file.
        let mut decoder = flate2::read::MultiGzDecoder::new(&bytes[..]);
        let mut inflated = Vec::new();
        decoder.read_to_end(&mut inflated)?;
        Ok(decode_log_bytes(inflated))
    } else {
        Ok(decode_log_bytes(bytes))
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
