//! Settings files people also edit by hand, changed as text. Only what Lanes adds or takes out
//! changes: comments, key order, indentation and line endings stay as they were, and taking an
//! addition back out gives the file back byte for byte. Gemini CLI's and VS Code's settings allow
//! comments (VS Code's trailing commas too), so they're read as JSON with comments.

use serde::Serialize;
use serde_json::{Map, Value};

/// A settings file: its text, and where each value in it is.
#[derive(Debug, Clone)]
pub struct Doc {
    text: String,
    root: Node,
    /// The file's line ending and one level of its indentation.
    nl: &'static str,
    unit: String,
}

#[derive(Debug, Clone)]
struct Node {
    start: usize,
    end: usize,
    value: Value,
    kids: Kids,
}

#[derive(Debug, Clone)]
enum Kids {
    None,
    /// Each member: its key, where the key starts, and its value.
    Object(Vec<(String, usize, Node)>),
    Array(Vec<Node>),
}

impl Doc {
    /// Read `text` as JSON with comments. An empty file reads as `{}`.
    pub fn parse(text: &str) -> Result<Doc, String> {
        let text = if text.trim().is_empty() { "{}" } else { text };
        let root = parse(text)?;
        let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
        Ok(Doc {
            unit: indent_unit(text),
            text: text.to_string(),
            root,
            nl,
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn value(&self) -> &Value {
        &self.root.value
    }

    /// The value at `path` (object keys, outermost first).
    pub fn get(&self, path: &[&str]) -> Option<&Value> {
        self.node(path).map(|n| &n.value)
    }

    fn node(&self, path: &[&str]) -> Option<&Node> {
        let mut node = &self.root;
        for key in path {
            let Kids::Object(members) = &node.kids else {
                return None;
            };
            // The last of a repeated key wins, as in every JSON reader.
            node = members
                .iter()
                .rev()
                .find(|(k, _, _)| k == key)
                .map(|m| &m.2)?;
        }
        Some(node)
    }

    /// The text of the value at `path`, exactly as it's written.
    pub fn raw(&self, path: &[&str]) -> Option<&str> {
        self.node(path).map(|n| &self.text[n.start..n.end])
    }

    /// Add `key: value` as the last member of the object at `parent`.
    pub fn insert(&mut self, parent: &[&str], key: &str, value: &Value) -> Result<(), String> {
        let node = self.node(parent).ok_or("no such object")?;
        let Kids::Object(members) = &node.kids else {
            return Err(format!("`{}` isn't an object", parent.join(".")));
        };
        let key = serde_json::to_string(key).expect("a key serializes");
        let (start, end) = (node.start, node.end);
        let after = members.last().map(|(_, k, v)| (*k, v.end));
        self.add(
            start,
            end,
            after,
            &|d: &Doc, indent: Option<&str>| match indent {
                None => format!("{key}: {}", compact(value)),
                Some(indent) => format!("{key}: {}", d.render(value, indent)),
            },
        )
    }

    /// Add `value` as the last element of the array at `path`.
    pub fn push(&mut self, path: &[&str], value: &Value) -> Result<(), String> {
        let node = self.node(path).ok_or("no such array")?;
        let Kids::Array(items) = &node.kids else {
            return Err(format!("`{}` isn't an array", path.join(".")));
        };
        let (start, end) = (node.start, node.end);
        let after = items.last().map(|v| (v.start, v.end));
        self.add(
            start,
            end,
            after,
            &|d: &Doc, indent: Option<&str>| match indent {
                None => compact(value),
                Some(indent) => d.render(value, indent),
            },
        )
    }

    /// Add an entry (made by `entry`, given its indentation, or `None` on one line) at the end of
    /// the object or array between `start` and `end`, whose last entry starts and ends at `last`.
    fn add(
        &mut self,
        start: usize,
        end: usize,
        last: Option<(usize, usize)>,
        entry: &dyn Fn(&Doc, Option<&str>) -> String,
    ) -> Result<(), String> {
        let base = line_indent(&self.text, start).to_string();
        let inner = format!("{base}{}", self.unit);
        let nl = self.nl;
        match last {
            // On its own line, like the entry before it; or on the same line, when the whole
            // object is on one line.
            Some((last_start, last_end)) => {
                let text = if !self.text[start..end].contains('\n') {
                    format!(", {}", entry(self, None))
                } else {
                    let indent = match own_line(&self.text, last_start) {
                        true => line_indent(&self.text, last_start).to_string(),
                        false => inner,
                    };
                    format!(",{nl}{indent}{}", entry(self, Some(&indent)))
                };
                self.splice(last_end, last_end, &text)
            }
            None => {
                let inside = &self.text[start + 1..end - 1];
                if inside.trim().is_empty() {
                    let text = format!("{nl}{inner}{}{nl}{base}", entry(self, Some(&inner)));
                    self.splice(start + 1, end - 1, &text)
                } else {
                    // Only comments inside: the entry goes after them.
                    let at = start + 1 + inside.trim_end().len();
                    let text = format!("{nl}{inner}{}", entry(self, Some(&inner)));
                    self.splice(at, at, &text)
                }
            }
        }
    }

    /// Replace the value at `path` with `raw`, text written as is.
    pub fn replace(&mut self, path: &[&str], raw: &str) -> Result<(), String> {
        let node = self.node(path).ok_or("no such value")?;
        let (start, end) = (node.start, node.end);
        self.splice(start, end, raw)
    }

    /// Take out the member at `path`. The exact opposite of `insert`, when it's still last.
    pub fn remove(&mut self, path: &[&str]) -> Result<bool, String> {
        let Some((key, parent)) = path.split_last() else {
            return Ok(false);
        };
        let Some(node) = self.node(parent) else {
            return Ok(false);
        };
        let Kids::Object(members) = &node.kids else {
            return Ok(false);
        };
        let Some(i) = members.iter().rposition(|(k, _, _)| k == key) else {
            return Ok(false);
        };
        let spans: Vec<(usize, usize)> = members.iter().map(|(_, k, v)| (*k, v.end)).collect();
        let (start, end) = (node.start, node.end);
        self.cut(start, end, &spans, i)?;
        Ok(true)
    }

    /// Take out each element of the array at `path` for which `ours` is true. Returns how many.
    pub fn remove_items(
        &mut self,
        path: &[&str],
        ours: impl Fn(&Value) -> bool,
    ) -> Result<usize, String> {
        let mut removed = 0;
        loop {
            let Some(node) = self.node(path) else {
                return Ok(removed);
            };
            let Kids::Array(items) = &node.kids else {
                return Ok(removed);
            };
            let Some(i) = items.iter().rposition(|v| ours(&v.value)) else {
                return Ok(removed);
            };
            let spans: Vec<(usize, usize)> = items.iter().map(|v| (v.start, v.end)).collect();
            let (start, end) = (node.start, node.end);
            self.cut(start, end, &spans, i)?;
            removed += 1;
        }
    }

    /// Cut entry `i` (of `spans`) out of the object or array between `start` and `end`.
    fn cut(
        &mut self,
        start: usize,
        end: usize,
        spans: &[(usize, usize)],
        i: usize,
    ) -> Result<(), String> {
        let (from, to) = spans[i];
        if i > 0 {
            // From the end of the entry before: that's where `add` put the comma and newline.
            return self.splice(spans[i - 1].1, to, "");
        }
        if let Some(&(next, _)) = spans.get(1) {
            return self.splice(from, next, "");
        }
        // The only entry. A trailing comma goes with it.
        let rest = &self.text[to..end - 1];
        let to = match rest.trim_start().starts_with(',') {
            true => to + rest.len() - rest.trim_start().len() + 1,
            false => to,
        };
        let before = &self.text[start + 1..from];
        if before.trim().is_empty() && self.text[to..end - 1].trim().is_empty() {
            return self.splice(start + 1, end - 1, "");
        }
        let at = start + 1 + before.trim_end().len();
        self.splice(at, to, "")
    }

    fn splice(&mut self, from: usize, to: usize, with: &str) -> Result<(), String> {
        let mut text = String::with_capacity(self.text.len() + with.len());
        text.push_str(&self.text[..from]);
        text.push_str(with);
        text.push_str(&self.text[to..]);
        self.root = parse(&text).map_err(|e| format!("an edit went wrong ({e})"))?;
        self.text = text;
        Ok(())
    }

    /// `value` written the way this file is: its indentation, its line endings, each line after
    /// the first indented by `indent`.
    fn render(&self, value: &Value, indent: &str) -> String {
        let mut out = Vec::new();
        let format = serde_json::ser::PrettyFormatter::with_indent(self.unit.as_bytes());
        value
            .serialize(&mut serde_json::Serializer::with_formatter(
                &mut out, format,
            ))
            .expect("JSON serializes");
        let text = String::from_utf8(out).expect("JSON is UTF-8");
        text.replace('\n', &format!("{}{indent}", self.nl))
    }
}

fn compact(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON serializes")
}

/// The leading spaces or tabs of the line `at` is on.
fn line_indent(text: &str, at: usize) -> &str {
    let line = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let rest = &text[line..];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

/// Whether nothing but indentation comes before `at` on its line.
fn own_line(text: &str, at: usize) -> bool {
    let line = text[..at].rfind('\n').map_or(0, |i| i + 1);
    text[line..at].trim().is_empty()
}

/// One level of indentation: the first indented line's, or two spaces.
fn indent_unit(text: &str) -> String {
    for line in text.lines().skip(1) {
        let indent = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
        if !indent.is_empty() && !line.trim().is_empty() {
            return if indent.starts_with('\t') {
                "\t".into()
            } else {
                indent.into()
            };
        }
    }
    "  ".into()
}

fn parse(text: &str) -> Result<Node, String> {
    let mut p = Parser {
        s: text.as_bytes(),
        text,
        i: 0,
    };
    if text.starts_with('\u{feff}') {
        p.i = '\u{feff}'.len_utf8();
    }
    p.space()?;
    let node = p.value()?;
    p.space()?;
    if p.i < p.s.len() {
        return Err(p.error("more after the end"));
    }
    Ok(node)
}

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn error(&self, what: &str) -> String {
        let before = &self.text[..self.i.min(self.text.len())];
        let line = before.matches('\n').count() + 1;
        let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
        format!("{what} at line {line}, column {column}")
    }

    /// Skip whitespace and comments.
    fn space(&mut self) -> Result<(), String> {
        loop {
            match self.s.get(self.i) {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.i += 1,
                Some(b'/') if self.s.get(self.i + 1) == Some(&b'/') => {
                    while self.i < self.s.len() && self.s[self.i] != b'\n' {
                        self.i += 1;
                    }
                }
                Some(b'/') if self.s.get(self.i + 1) == Some(&b'*') => {
                    let end = self.text[self.i + 2..]
                        .find("*/")
                        .ok_or_else(|| self.error("a comment that never ends"))?;
                    self.i += 2 + end + 2;
                }
                _ => return Ok(()),
            }
        }
    }

    fn value(&mut self) -> Result<Node, String> {
        let start = self.i;
        let (value, kids) = match self.s.get(self.i) {
            Some(b'{') => self.object()?,
            Some(b'[') => self.array()?,
            Some(b'"') => (Value::String(self.string()?), Kids::None),
            Some(b't' | b'f' | b'n') => {
                let word = ["true", "false", "null"]
                    .into_iter()
                    .find(|w| self.text[self.i..].starts_with(w))
                    .ok_or_else(|| self.error("an unexpected word"))?;
                self.i += word.len();
                (serde_json::from_str(word).expect("a JSON word"), Kids::None)
            }
            Some(b'-' | b'0'..=b'9') => {
                while self
                    .s
                    .get(self.i)
                    .is_some_and(|c| c.is_ascii_digit() || b"+-.eE".contains(c))
                {
                    self.i += 1;
                }
                let number = serde_json::from_str(&self.text[start..self.i])
                    .map_err(|_| self.error("a number that isn't one"))?;
                (number, Kids::None)
            }
            Some(_) => return Err(self.error("an unexpected character")),
            None => return Err(self.error("the end of the file")),
        };
        Ok(Node {
            start,
            end: self.i,
            value,
            kids,
        })
    }

    fn string(&mut self) -> Result<String, String> {
        let start = self.i;
        self.i += 1;
        loop {
            match self.s.get(self.i) {
                Some(b'\\') => self.i += 2,
                Some(b'"') => break,
                Some(_) => self.i += 1,
                None => return Err(self.error("a string that never ends")),
            }
        }
        self.i += 1;
        serde_json::from_str(&self.text[start..self.i]).map_err(|e| self.error(&e.to_string()))
    }

    fn object(&mut self) -> Result<(Value, Kids), String> {
        self.i += 1;
        let (mut map, mut members) = (Map::new(), Vec::new());
        loop {
            self.space()?;
            match self.s.get(self.i) {
                Some(b'}') => break,
                Some(b'"') => {}
                _ => return Err(self.error("expected a key or `}`")),
            }
            let key_start = self.i;
            let key = self.string()?;
            self.space()?;
            if self.s.get(self.i) != Some(&b':') {
                return Err(self.error("expected `:`"));
            }
            self.i += 1;
            self.space()?;
            let node = self.value()?;
            map.insert(key.clone(), node.value.clone());
            members.push((key, key_start, node));
            self.space()?;
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => break,
                _ => return Err(self.error("expected `,` or `}`")),
            }
        }
        self.i += 1;
        Ok((Value::Object(map), Kids::Object(members)))
    }

    fn array(&mut self) -> Result<(Value, Kids), String> {
        self.i += 1;
        let (mut values, mut items) = (Vec::new(), Vec::new());
        loop {
            self.space()?;
            if self.s.get(self.i) == Some(&b']') {
                break;
            }
            let node = self.value()?;
            values.push(node.value.clone());
            items.push(node);
            self.space()?;
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => break,
                _ => return Err(self.error("expected `,` or `]`")),
            }
        }
        self.i += 1;
        Ok((Value::Array(values), Kids::Array(items)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Insert, then remove: the text is back as it was, byte for byte.
    fn round_trip(text: &str, parent: &[&str], key: &str, value: Value) -> String {
        let path: Vec<&str> = parent.iter().copied().chain([key]).collect();
        let mut doc = Doc::parse(text).unwrap();
        doc.insert(parent, key, &value).unwrap();
        let added = doc.text().to_string();
        assert_eq!(doc.get(&path), Some(&value), "{added}");
        doc.remove(&path).unwrap();
        assert_eq!(doc.text(), text, "after taking it back out of:\n{added}");
        added
    }

    #[test]
    fn reads_comments_trailing_commas_and_a_byte_order_mark() {
        let doc = Doc::parse(
            "\u{feff}{\n  // a comment\n  \"a\": [1, 2,], /* another */\n  \"b\": {\"c\": \"x\\\"y\"},\n}\n",
        )
        .unwrap();
        assert_eq!(doc.value(), &json!({"a": [1, 2], "b": {"c": "x\"y"}}));
        assert_eq!(doc.raw(&["b", "c"]), Some("\"x\\\"y\""));
        assert!(Doc::parse("{\"a\": }").unwrap_err().contains("line 1"));
        assert!(Doc::parse("{\"a\": 1} x").is_err());
        assert!(Doc::parse("/* never ends").is_err());
        assert_eq!(Doc::parse("  \n").unwrap().value(), &json!({}));
    }

    #[test]
    fn an_insert_matches_the_file_and_comes_back_out_exactly() {
        // Four spaces and comments, like VS Code's settings.
        let added = round_trip(
            "{\n    // Editor\n    \"editor.fontSize\": 14 // big\n}\n",
            &[],
            "chat.useAgentsMdFile",
            json!(true),
        );
        assert_eq!(
            added,
            "{\n    // Editor\n    \"editor.fontSize\": 14,\n    \"chat.useAgentsMdFile\": true // big\n}\n"
        );
        // Nested, with Windows line endings.
        let added = round_trip(
            "{\r\n  \"a\": 1\r\n}\r\n",
            &[],
            "hooks",
            json!({"X": [{"command": "y"}]}),
        );
        assert_eq!(
            added,
            "{\r\n  \"a\": 1,\r\n  \"hooks\": {\r\n    \"X\": [\r\n      {\r\n        \"command\": \"y\"\r\n      }\r\n    ]\r\n  }\r\n}\r\n"
        );
        // Tabs, a trailing comma, an object on one line, an empty object, comments only.
        round_trip("{\n\t\"a\": 1,\n}", &[], "b", json!([1]));
        round_trip("{\"a\": {\"b\": 1}}", &["a"], "c", json!("d"));
        round_trip("{}", &[], "a", json!({"b": []}));
        round_trip("{\n  \"a\": {}\n}", &["a"], "b", json!(2));
        round_trip("{\n  // nothing yet\n}\n", &[], "a", json!(1));
    }

    #[test]
    fn pushing_and_removing_items() {
        let text = "{\n  \"fileName\": [\"GEMINI.md\"],\n  \"list\": [\n    1\n  ]\n}";
        let mut doc = Doc::parse(text).unwrap();
        doc.push(&["fileName"], &json!("AGENTS.md")).unwrap();
        doc.push(&["list"], &json!({"n": 2})).unwrap();
        assert_eq!(
            doc.text(),
            "{\n  \"fileName\": [\"GEMINI.md\", \"AGENTS.md\"],\n  \"list\": [\n    1,\n    {\n      \"n\": 2\n    }\n  ]\n}"
        );
        assert_eq!(doc.remove_items(&["list"], |v| v.is_object()), Ok(1));
        assert_eq!(doc.remove_items(&["fileName"], |v| v == "AGENTS.md"), Ok(1));
        assert_eq!(doc.text(), text);

        // An entry in the middle, or the first of several, goes with its comma.
        let mut doc = Doc::parse("[1, 2, 3]").unwrap();
        doc.remove_items(&[], |v| v == 2).unwrap();
        assert_eq!(doc.text(), "[1, 3]");
        doc.remove_items(&[], |v| v == 1).unwrap();
        assert_eq!(doc.text(), "[3]");
        doc.remove_items(&[], |v| v == 3).unwrap();
        assert_eq!(doc.text(), "[]");
        let mut doc = Doc::parse("{\"a\": 1, \"b\": 2,}").unwrap();
        doc.remove(&["b"]).unwrap();
        doc.remove(&["a"]).unwrap();
        assert_eq!(doc.value(), &json!({}));
        assert!(!doc.remove(&["a"]).unwrap());
    }

    #[test]
    fn replacing_a_value_keeps_everything_around_it() {
        let mut doc = Doc::parse("{ \"fileName\": \"CONTEXT.md\" /* mine */ }").unwrap();
        doc.replace(&["fileName"], "[\"CONTEXT.md\", \"AGENTS.md\"]")
            .unwrap();
        assert_eq!(
            doc.text(),
            "{ \"fileName\": [\"CONTEXT.md\", \"AGENTS.md\"] /* mine */ }"
        );
        assert!(doc.replace(&["fileName"], "[").is_err(), "never invalid");
    }
}
