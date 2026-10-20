//! Compose variable interpolation: `$VAR`, `${VAR}`, `${VAR:-d}`,
//! `${VAR-d}`, `${VAR:?err}`, `${VAR?err}` and `$$`. Values come from the
//! stack's variables only.

use super::ComposeIssue;
use std::collections::BTreeMap;

/// Substitutes variables in `s`. Defaults are interpolated in turn;
/// substituted values never are. An unset variable with no default becomes
/// an empty string and adds a warning, as in Compose.
pub fn interpolate(
    s: &str,
    vars: &BTreeMap<String, String>,
    path: &str,
    warnings: &mut Vec<ComposeIssue>,
) -> Result<String, ComposeIssue> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        match chars.get(i + 1) {
            Some('$') => {
                out.push('$');
                i += 2;
            }
            Some('{') => {
                let close = matching_brace(&chars, i + 2)
                    .ok_or_else(|| ComposeIssue::new(path, format!("unclosed '${{' in \"{s}\"")))?;
                let inner: String = chars[i + 2..close].iter().collect();
                out.push_str(&expand(&inner, vars, path, warnings)?);
                i = close + 1;
            }
            Some(c) if c.is_ascii_alphabetic() || *c == '_' => {
                let start = i + 1;
                let mut end = start;
                while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_')
                {
                    end += 1;
                }
                let name: String = chars[start..end].iter().collect();
                out.push_str(&lookup(&name, vars, path, warnings));
                i = end;
            }
            _ => {
                out.push('$');
                i += 1;
            }
        }
    }
    Ok(out)
}

/// The index of the `}` closing a `${` whose body starts at `from`.
fn matching_brace(chars: &[char], from: usize) -> Option<usize> {
    let mut depth = 1;
    let mut i = from;
    while i < chars.len() {
        match chars[i] {
            '$' if chars.get(i + 1) == Some(&'{') => {
                depth += 1;
                i += 2;
                continue;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn expand(
    inner: &str,
    vars: &BTreeMap<String, String>,
    path: &str,
    warnings: &mut Vec<ComposeIssue>,
) -> Result<String, ComposeIssue> {
    let name_end = inner
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(inner.len());
    let name = &inner[..name_end];
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(ComposeIssue::new(
            path,
            format!("invalid variable reference '${{{inner}}}'"),
        ));
    }
    let rest = &inner[name_end..];
    let value = vars.get(name);
    let (op, arg) = if let Some(a) = rest.strip_prefix(":-") {
        (":-", a)
    } else if let Some(a) = rest.strip_prefix(":?") {
        (":?", a)
    } else if let Some(a) = rest.strip_prefix('-') {
        ("-", a)
    } else if let Some(a) = rest.strip_prefix('?') {
        ("?", a)
    } else if rest.is_empty() {
        ("", "")
    } else {
        return Err(ComposeIssue::new(
            path,
            format!("invalid variable reference '${{{inner}}}'"),
        ));
    };
    let unset_or_empty = value.is_none_or(|v| v.is_empty());
    Ok(match op {
        ":-" if unset_or_empty => interpolate(arg, vars, path, warnings)?,
        "-" if value.is_none() => interpolate(arg, vars, path, warnings)?,
        ":?" if unset_or_empty => return Err(required(name, arg, path, vars, warnings)?),
        "?" if value.is_none() => return Err(required(name, arg, path, vars, warnings)?),
        "" => lookup(name, vars, path, warnings),
        _ => value.cloned().unwrap_or_default(),
    })
}

fn required(
    name: &str,
    msg: &str,
    path: &str,
    vars: &BTreeMap<String, String>,
    warnings: &mut Vec<ComposeIssue>,
) -> Result<ComposeIssue, ComposeIssue> {
    let msg = interpolate(msg, vars, path, warnings)?;
    let msg = if msg.is_empty() {
        format!("required variable {name} is not set")
    } else {
        format!("required variable {name} is not set: {msg}")
    };
    Ok(ComposeIssue::new(path, msg))
}

fn lookup(
    name: &str,
    vars: &BTreeMap<String, String>,
    path: &str,
    warnings: &mut Vec<ComposeIssue>,
) -> String {
    match vars.get(name) {
        Some(v) => v.clone(),
        None => {
            warnings.push(ComposeIssue::new(
                path,
                format!("variable {name} is not set; using an empty string"),
            ));
            String::new()
        }
    }
}

/// `KEY=value` lines into a map. Blank lines and `#` comments are skipped.
/// The value is everything after the first `=`, verbatim.
pub fn parse_variables(lines: &[String]) -> Result<BTreeMap<String, String>, ComposeIssue> {
    let mut out = BTreeMap::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once('=').ok_or_else(|| {
            ComposeIssue::new(
                format!("variables[{i}]"),
                format!("'{line}' is not KEY=value"),
            )
        })?;
        let k = k.trim();
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(ComposeIssue::new(
                format!("variables[{i}]"),
                format!("invalid variable name '{k}'"),
            ));
        }
        if out.insert(k.to_string(), v.to_string()).is_some() {
            return Err(ComposeIssue::new(
                format!("variables[{i}]"),
                format!("variable {k} is set twice"),
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn run(
        s: &str,
        v: &BTreeMap<String, String>,
    ) -> (Result<String, ComposeIssue>, Vec<ComposeIssue>) {
        let mut w = Vec::new();
        let r = interpolate(s, v, "services.web.image", &mut w);
        (r, w)
    }

    #[test]
    fn plain_and_braced() {
        let v = vars(&[("A", "x")]);
        assert_eq!(run("$A", &v).0.unwrap(), "x");
        assert_eq!(run("${A}", &v).0.unwrap(), "x");
        assert_eq!(run("a-${A}-b", &v).0.unwrap(), "a-x-b");
    }

    #[test]
    fn colon_dash_default() {
        assert_eq!(run("${A:-d}", &vars(&[])).0.unwrap(), "d");
        assert_eq!(run("${A:-d}", &vars(&[("A", "")])).0.unwrap(), "d");
    }

    #[test]
    fn dash_default_only_when_unset() {
        assert_eq!(run("${A-d}", &vars(&[("A", "")])).0.unwrap(), "");
        assert_eq!(run("${A-d}", &vars(&[])).0.unwrap(), "d");
    }

    #[test]
    fn required_errors() {
        let e = run("${A:?need A}", &vars(&[])).0.unwrap_err();
        assert!(e.message.contains("need A"), "{}", e.message);
        assert_eq!(e.path, "services.web.image");
        assert!(run("${A?x}", &vars(&[])).0.is_err());
        assert_eq!(run("${A?x}", &vars(&[("A", "")])).0.unwrap(), "");
    }

    #[test]
    fn dollar_escape() {
        assert_eq!(run("$$", &vars(&[])).0.unwrap(), "$");
        assert_eq!(run("a$$b", &vars(&[])).0.unwrap(), "a$b");
    }

    #[test]
    fn unset_warns_and_is_empty() {
        let (r, w) = run("${B}", &vars(&[]));
        assert_eq!(r.unwrap(), "");
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].message, "variable B is not set; using an empty string");
    }

    #[test]
    fn unclosed_is_error() {
        assert!(run("${A", &vars(&[("A", "x")])).0.is_err());
    }

    #[test]
    fn nested_default() {
        assert_eq!(run("${A:-${B:-z}}", &vars(&[])).0.unwrap(), "z");
    }

    #[test]
    fn substituted_values_are_verbatim() {
        let v = vars(&[("P", "a$b \"c\" ")]);
        let (r, w) = run("${P}", &v);
        assert_eq!(r.unwrap(), "a$b \"c\" ");
        assert!(w.is_empty());
    }

    #[test]
    fn variables_parse() {
        let lines = vec![
            "K=a=b".to_string(),
            "".into(),
            "# c".into(),
            "P=a$b \"c\" ".into(),
        ];
        let m = parse_variables(&lines).unwrap();
        assert_eq!(m["K"], "a=b");
        assert_eq!(m["P"], "a$b \"c\" ");
        assert!(parse_variables(&["NOEQ".into()]).is_err());
        let dup = parse_variables(&["A=1".into(), "A=2".into()]).unwrap_err();
        assert_eq!(dup.message, "variable A is set twice");
    }
}
