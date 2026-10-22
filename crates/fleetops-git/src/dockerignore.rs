//! `.dockerignore` matching, as the Docker CLI applies it before sending a
//! build context: one pattern per line, `#` comments, `!` re-includes, the
//! last matching pattern wins, and a pattern that matches a directory
//! matches everything under it.

struct Pattern {
    parts: Vec<String>,
    exclude: bool,
}

pub struct Matcher {
    patterns: Vec<Pattern>,
}

impl Matcher {
    pub fn new(text: &str) -> Self {
        let patterns = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter_map(|l| {
                let (exclude, p) = match l.strip_prefix('!') {
                    Some(rest) => (true, rest.trim()),
                    None => (false, l),
                };
                let parts: Vec<String> = p
                    .split('/')
                    .filter(|s| !s.is_empty() && *s != ".")
                    .map(String::from)
                    .collect();
                (!parts.is_empty()).then_some(Pattern { parts, exclude })
            })
            .collect();
        Self { patterns }
    }

    /// Whether any `!` pattern exists, so an ignored directory may still
    /// hold files to send.
    pub fn has_exceptions(&self) -> bool {
        self.patterns.iter().any(|p| p.exclude)
    }

    /// Whether `path` (relative to the context, `/`-separated) is left out.
    pub fn ignored(&self, path: &str) -> bool {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut out = false;
        for p in &self.patterns {
            // A pattern matches the path or any of its parent directories.
            let hit = (1..=segs.len()).any(|n| match_parts(&p.parts, &segs[..n]));
            if hit {
                out = !p.exclude;
            }
        }
        out
    }
}

fn match_parts(pat: &[String], segs: &[&str]) -> bool {
    match pat.split_first() {
        None => segs.is_empty(),
        Some((first, rest)) if first == "**" => {
            (0..=segs.len()).any(|i| match_parts(rest, &segs[i..]))
        }
        Some((first, rest)) => match segs.split_first() {
            Some((s, srest)) => glob(first.as_bytes(), s.as_bytes()) && match_parts(rest, srest),
            None => false,
        },
    }
}

/// One path segment against `*`, `?`, `[...]` and `\` escapes.
fn glob(p: &[u8], s: &[u8]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| glob(rest, &s[i..])),
        Some((b'?', rest)) => !s.is_empty() && glob(rest, &s[1..]),
        Some((b'[', rest)) => {
            let Some(end) = rest.iter().position(|&c| c == b']') else {
                return s.first() == Some(&b'[') && glob(rest, &s[1..]);
            };
            let Some((&c, srest)) = s.split_first() else {
                return false;
            };
            let (neg, class) = match rest[..end].split_first() {
                Some((b'^' | b'!', cl)) => (true, cl),
                _ => (false, &rest[..end]),
            };
            let mut hit = false;
            let mut i = 0;
            while i < class.len() {
                if i + 2 < class.len() && class[i + 1] == b'-' {
                    hit |= class[i] <= c && c <= class[i + 2];
                    i += 3;
                } else {
                    hit |= class[i] == c;
                    i += 1;
                }
            }
            hit != neg && glob(&rest[end + 1..], srest)
        }
        Some((b'\\', rest)) if !rest.is_empty() => {
            s.first() == Some(&rest[0]) && glob(&rest[1..], &s[1..])
        }
        Some((&c, rest)) => s.first() == Some(&c) && glob(rest, &s[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_rules() {
        let cases: &[(&str, &str, bool)] = &[
            ("node_modules", "node_modules/x/y.js", true),
            ("node_modules", "src/node_modules", false),
            ("*.log", "a.log", true),
            ("*.log", "d/a.log", false),
            ("**/*.log", "d/e/a.log", true),
            ("**/*.log", "a.log", true),
            ("*.log\n!keep.log", "keep.log", false),
            ("*.log\n!keep.log", "other.log", true),
            ("/tmp", "tmp/x", true),
            ("./tmp", "tmp", true),
            ("# c\n", "# c", false),
            ("docs/", "docs/a.md", true),
            ("a/**/b", "a/x/y/b", true),
            ("a/**/b", "a/b", true),
            ("file?.txt", "file1.txt", true),
            ("file[0-9].txt", "filea.txt", false),
            ("file[!0-9].txt", "filea.txt", true),
            ("secret\n!secret/public", "secret/public/x", false),
            ("secret\n!secret/public", "secret/private", true),
        ];
        for (pats, path, want) in cases {
            assert_eq!(Matcher::new(pats).ignored(path), *want, "{pats:?} {path}");
        }
        assert!(Matcher::new("a\n!b").has_exceptions());
        assert!(!Matcher::new("a").has_exceptions());
    }
}
