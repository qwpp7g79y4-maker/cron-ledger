//! Bound on every file and stdin stream this program loads into memory.
//!
//! The measured input of 104857600 bytes (100 MiB) stays under this.
//! A larger input is refused before its body is kept.

use std::io::{self, Read};
use std::path::Path;

/// Largest input accepted per file or stdin stream, in bytes (256 MiB).
pub const MAX_INPUT_BYTES: u64 = 256 * 1024 * 1024;

fn too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::FileTooLarge,
        format!("file is larger than {MAX_INPUT_BYTES} bytes"),
    )
}

/// Read `path` up to [`MAX_INPUT_BYTES`].
///
/// A regular file past the limit is refused from its size, so the body is
/// not loaded. Anything else is read at most one byte past the limit.
pub fn read_capped_file(path: &Path) -> io::Result<Vec<u8>> {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.is_file() && meta.len() > MAX_INPUT_BYTES {
            return Err(too_large());
        }
    }
    let file = std::fs::File::open(path)?;
    read_capped_reader(file)
}

/// Read at most one byte past [`MAX_INPUT_BYTES`]. A longer stream is refused
/// and the rest is not kept.
pub fn read_capped_reader(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    reader.take(MAX_INPUT_BYTES + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_INPUT_BYTES {
        return Err(too_large());
    }
    Ok(buf)
}

/// The message for a failed read. A size refusal names the limit.
/// `stdin` selects the hint used for a pipe into cron-ledger.
pub fn explain_read(name: &str, err: &io::Error, stdin: bool) -> String {
    if err.kind() == io::ErrorKind::FileTooLarge {
        format!(
            "cannot read {name}: file is larger than {MAX_INPUT_BYTES} bytes; split the file or pass a smaller one"
        )
    } else if stdin {
        format!("cannot read {name}: {err}; check the command that pipes into cron-ledger")
    } else {
        format!("cannot read {name}: {err}; check that the path exists and is readable")
    }
}
