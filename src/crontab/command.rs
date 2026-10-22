//! vixie-cron's `%` handling: an unescaped `%` in the command field splits
//! the real command from data piped to it on stdin, and every later
//! unescaped `%` becomes a newline within that stdin data. `\%` is a literal
//! `%` with the backslash removed.

/// Split a raw command field into the command to run and, if any unescaped
/// `%` was present, the text to feed it on stdin.
pub fn split_command_stdin(raw: &str) -> (String, Option<String>) {
    let mut command = String::new();
    let mut stdin_buf = String::new();
    let mut in_stdin = false;
    let mut chars = raw.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'%') {
            chars.next();
            if in_stdin {
                stdin_buf.push('%');
            } else {
                command.push('%');
            }
            continue;
        }
        if c == '%' {
            if in_stdin {
                stdin_buf.push('\n');
            } else {
                in_stdin = true;
            }
            continue;
        }
        if in_stdin {
            stdin_buf.push(c);
        } else {
            command.push(c);
        }
    }

    (command, if in_stdin { Some(stdin_buf) } else { None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_percent_is_passthrough() {
        assert_eq!(
            split_command_stdin("echo hello"),
            ("echo hello".to_string(), None)
        );
    }

    #[test]
    fn escaped_percent_is_literal() {
        assert_eq!(
            split_command_stdin(r"date +\%F"),
            ("date +%F".to_string(), None)
        );
    }

    #[test]
    fn bare_percent_splits_stdin() {
        assert_eq!(
            split_command_stdin("cat%hello%world"),
            ("cat".to_string(), Some("hello\nworld".to_string()))
        );
    }

    #[test]
    fn mixed_escaped_and_bare() {
        assert_eq!(
            split_command_stdin(r"echo a\%b%c\%d"),
            ("echo a%b".to_string(), Some("c%d".to_string()))
        );
    }

    #[test]
    fn trailing_backslash_is_kept_literal() {
        // A backslash not followed by '%' is not an escape sequence in
        // vixie-cron; it is passed through untouched.
        assert_eq!(
            split_command_stdin(r"echo a\nb"),
            (r"echo a\nb".to_string(), None)
        );
    }
}
