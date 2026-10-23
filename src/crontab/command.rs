//! vixie-cron's `%` handling, matching cronie's `do_command()`.
//!
//! An unescaped `%` splits the command from the text piped to it on
//! stdin. A backslash escapes only the next character, and only `%` is
//! special. In the command, an even run of backslashes before `%` leaves
//! that `%` as the separator and keeps every backslash; an odd run
//! escapes the `%` (one backslash is consumed, the others stay) and does
//! not split.
//!
//! The stdin text uses cronie's second loop, which is not the same
//! rule: `\%` becomes `%`, a bare `%` becomes a newline, and `\\%`
//! becomes one literal backslash followed by a literal percent. A
//! backslash that escapes another backslash leaves that second
//! backslash armed for the character after it.

/// Split a raw command field into the command to run and, if any unescaped
/// `%` was present, the text to feed it on stdin. See the module comment
/// for the command/stdin difference.
pub fn split_command_stdin(raw: &str) -> (String, Option<String>) {
    let mut command = String::new();
    let mut escaped = false;

    for (idx, c) in raw.char_indices() {
        if escaped {
            escaped = false;
            if c != '%' {
                command.push('\\');
            }
            command.push(c);
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == '%' {
            let rest = &raw[idx + c.len_utf8()..];
            return (command, Some(translate_stdin(rest)));
        }
        command.push(c);
    }
    if escaped {
        command.push('\\');
    }

    (command, None)
}

/// Cronie's stdin copier (`do_command.c`, the loop that writes the
/// grandchild's stdin). `\%` becomes `%`, `%` becomes a newline, and
/// `\x` stays `\x` for every other `x`. Escaping a backslash emits one
/// backslash and leaves the second one armed, so `\\%` is `\%`, not a
/// newline.
fn translate_stdin(raw: &str) -> String {
    let mut out = String::new();
    let mut escaped = false;
    for mut ch in raw.chars() {
        if escaped {
            if ch != '%' {
                out.push('\\');
            }
        } else if ch == '%' {
            ch = '\n';
        }
        escaped = ch == '\\';
        if !escaped {
            out.push(ch);
        }
    }
    if escaped {
        out.push('\\');
    }
    out
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

    #[test]
    fn lone_trailing_backslash_is_kept_literal() {
        assert_eq!(
            split_command_stdin(r"echo a\"),
            (r"echo a\".to_string(), None)
        );
    }

    #[test]
    fn paired_backslashes_cancel_and_percent_still_splits() {
        // Two backslashes before `%` cancel each other out in cronie's
        // do_command(): the second backslash is the one that is
        // "escaped", not the `%`, so the `%` still separates command
        // from stdin.
        assert_eq!(
            split_command_stdin(r"echo a\\%hello"),
            (r"echo a\\".to_string(), Some("hello".to_string()))
        );
    }

    #[test]
    fn three_backslashes_before_percent_leave_it_escaped() {
        // Three backslashes are one escaped backslash plus one escape
        // for `%`. The `%` stays literal, so nothing is split off, and
        // the command keeps the remaining two backslashes.
        assert_eq!(
            split_command_stdin(r"echo a\\\%hello"),
            (r"echo a\\%hello".to_string(), None)
        );
    }

    #[test]
    fn paired_backslashes_in_stdin_stay_a_literal_percent() {
        // After the split, cronie's second loop does not treat `\\%`
        // as two literal backslashes plus a newline. The second
        // backslash stays armed and escapes the percent.
        assert_eq!(
            split_command_stdin(r"echo%a\\%b"),
            ("echo".to_string(), Some(r"a\%b".to_string()))
        );
        assert_eq!(
            split_command_stdin(r"echo%a\\\\%b"),
            ("echo".to_string(), Some(r"a\\\%b".to_string()))
        );
    }
}
