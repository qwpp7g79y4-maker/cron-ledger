//! Reading a log source from a file, transparently decompressing it if its
//! name ends in `.gz`. Never consulted implicitly: a `.gz` file is only
//! ever read because the caller named it explicitly on the command line.

use std::io::Read;
use std::path::Path;

/// Read `path` as UTF-8 text, gunzipping it first if its extension is
/// `gz`.
pub fn read_log_source(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    if path.extension().and_then(|e| e.to_str()) == Some("gz") {
        let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
        let mut out = String::new();
        decoder.read_to_string(&mut out)?;
        Ok(out)
    } else {
        String::from_utf8(bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
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
        assert_eq!(read_log_source(&path).unwrap(), "hello\n");
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
        assert_eq!(read_log_source(&path).unwrap(), "archived line\n");
        std::fs::remove_dir_all(&dir).ok();
    }
}
