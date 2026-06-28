//! A small shell-style tokenizer for REPL input: splits on whitespace, honoring
//! single quotes, double quotes, and backslash escapes. Lenient — an unterminated
//! quote simply takes the rest of the line. Shared by the executor and the
//! completer so they split a line the same way.

/// Splits `line` into argument tokens.
pub fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = line.chars();

    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            '\'' => {
                in_token = true;
                for d in chars.by_ref() {
                    if d == '\'' {
                        break;
                    }
                    current.push(d);
                }
            }
            '"' => {
                in_token = true;
                while let Some(d) = chars.next() {
                    match d {
                        '"' => break,
                        '\\' => {
                            if let Some(e) = chars.next() {
                                current.push(e);
                            }
                        }
                        other => current.push(other),
                    }
                }
            }
            '\\' => {
                in_token = true;
                if let Some(d) = chars.next() {
                    current.push(d);
                }
            }
            other => {
                in_token = true;
                current.push(other);
            }
        }
    }
    if in_token {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::tokenize;

    #[test]
    fn splits_on_whitespace() {
        assert_eq!(
            tokenize("market tickers BTC-USD"),
            ["market", "tickers", "BTC-USD"]
        );
    }

    #[test]
    fn empty_and_blank_yield_no_tokens() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn honors_quotes_and_escapes() {
        assert_eq!(tokenize(r#"a "b c" 'd e'"#), ["a", "b c", "d e"]);
        assert_eq!(tokenize(r"a\ b"), ["a b"]);
        assert_eq!(tokenize(r#""he said \"hi\"""#), [r#"he said "hi""#]);
    }

    #[test]
    fn empty_quotes_make_an_empty_token() {
        assert_eq!(tokenize("get ''"), ["get", ""]);
    }
}
