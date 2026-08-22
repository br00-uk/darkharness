//! Splits a command line into a program and its arguments.
//!
//! The harness never hands a whole command line to a shell unless the caller
//! sets `shell = true`. This module does the splitting that a shell would
//! otherwise do, so the child process still receives separate arguments and
//! no shell ever interprets a metacharacter.

use dark_contract::{ErrCode, Error, Result};

/// Which quote, if any, the scanner is inside.
#[derive(PartialEq, Eq)]
enum Quote {
    /// Not inside a quote.
    None,
    /// Inside a `'…'` span. Every character is literal.
    Single,
    /// Inside a `"…"` span. A backslash can escape `"` and `\`.
    Double,
}

/// Splits `line` into words, the way a POSIX shell would tokenize it.
///
/// Whitespace outside quotes separates words. Single quotes keep every
/// character literal. Double quotes allow a backslash to escape a `"` or a
/// `\`. A backslash outside quotes escapes the next character.
///
/// # Errors
///
/// Returns [`ErrCode::ToolInvalidArgs`] when a quote is not closed, or when
/// the line ends with a bare backslash.
pub(crate) fn split(line: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote = Quote::None;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Quote::Single => {
                if c == '\'' {
                    quote = Quote::None;
                } else {
                    current.push(c);
                }
            }
            Quote::Double => match c {
                '"' => quote = Quote::None,
                '\\' => match chars.peek() {
                    Some('"' | '\\') => current.push(chars.next().expect("peeked")),
                    _ => current.push('\\'),
                },
                other => current.push(other),
            },
            Quote::None => match c {
                ' ' | '\t' | '\n' | '\r' => {
                    if in_word {
                        words.push(std::mem::take(&mut current));
                        in_word = false;
                    }
                }
                '\'' => {
                    quote = Quote::Single;
                    in_word = true;
                }
                '"' => {
                    quote = Quote::Double;
                    in_word = true;
                }
                '\\' => {
                    in_word = true;
                    match chars.next() {
                        Some(escaped) => current.push(escaped),
                        None => {
                            return Err(Error::new(
                                ErrCode::ToolInvalidArgs,
                                "the command ends with a bare backslash",
                            ));
                        }
                    }
                }
                other => {
                    in_word = true;
                    current.push(other);
                }
            },
        }
    }

    if quote != Quote::None {
        return Err(Error::new(
            ErrCode::ToolInvalidArgs,
            "the command has an unclosed quote",
        ));
    }
    if in_word {
        words.push(current);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::split;
    use dark_contract::ErrCode;

    #[test]
    fn splits_on_plain_whitespace() {
        assert_eq!(
            split("cargo test --lib").unwrap(),
            vec!["cargo", "test", "--lib"]
        );
    }

    #[test]
    fn collapses_runs_of_whitespace() {
        assert_eq!(split("a   b\tc").unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn single_quotes_keep_spaces_literal() {
        assert_eq!(
            split("echo 'hello world'").unwrap(),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn double_quotes_keep_spaces_literal() {
        assert_eq!(
            split("echo \"hello world\"").unwrap(),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn single_quotes_do_not_interpret_backslash() {
        assert_eq!(split(r"echo 'a\b'").unwrap(), vec!["echo", r"a\b"]);
    }

    #[test]
    fn double_quotes_interpret_only_backslash_quote_and_backslash() {
        assert_eq!(
            split(r#"echo "a\"b\\c\d""#).unwrap(),
            vec!["echo", r#"a"b\c\d"#]
        );
    }

    #[test]
    fn a_bare_backslash_escapes_the_next_character_outside_quotes() {
        assert_eq!(split(r"a\ b").unwrap(), vec!["a b"]);
    }

    #[test]
    fn adjacent_quoted_and_unquoted_segments_join_into_one_word() {
        assert_eq!(
            split("foo'bar baz'qux").unwrap(),
            vec!["foobar bazqux"]
        );
    }

    #[test]
    fn an_empty_quote_produces_an_empty_word() {
        assert_eq!(split("a '' b").unwrap(), vec!["a", "", "b"]);
    }

    #[test]
    fn an_unclosed_single_quote_is_rejected() {
        let err = split("echo 'oops").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn an_unclosed_double_quote_is_rejected() {
        let err = split("echo \"oops").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn a_trailing_bare_backslash_is_rejected() {
        let err = split(r"echo \").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn an_empty_line_splits_to_no_words() {
        assert_eq!(split("").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn leading_and_trailing_whitespace_is_ignored() {
        assert_eq!(split("  ls -la  ").unwrap(), vec!["ls", "-la"]);
    }
}
