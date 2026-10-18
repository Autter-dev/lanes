//! `repo.outline`: a file's top-level definitions and their line numbers. Best effort: each
//! language is read line by line, not parsed, so only what starts at the left margin is found
//! (methods inside a class or `impl` aren't), and unusual formatting can hide a definition.

use serde::Serialize;
use std::path::Path;

/// One definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    /// 1-based.
    pub line: usize,
    /// The keyword that made it: `fn`, `struct`, `class`, `def`, `func`, `method`, ...
    pub kind: &'static str,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    /// TypeScript and JavaScript, which share their top-level syntax.
    TypeScript,
    Python,
    Go,
}

impl Language {
    /// The language a file is in, from its extension. `None` for languages without an outline.
    pub fn of(path: &Path) -> Option<Language> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "rs" => Language::Rust,
            "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => Language::TypeScript,
            "py" | "pyi" => Language::Python,
            "go" => Language::Go,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::TypeScript => "typescript",
            Language::Python => "python",
            Language::Go => "go",
        }
    }
}

pub fn outline(language: Language, text: &str) -> Vec<Item> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines = text
        .lines()
        .map(str::trim_end)
        .enumerate()
        .map(|(i, l)| (i + 1, l));
    match language {
        Language::Rust => lines.filter_map(rust).collect(),
        Language::TypeScript => lines.filter_map(typescript).collect(),
        Language::Python => lines.filter_map(python).collect(),
        Language::Go => go(lines),
    }
}

/// `s` after the keyword `w` and the spaces that follow it, when `s` starts with that word.
fn word<'a>(s: &'a str, w: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(w)?;
    rest.starts_with([' ', '\t']).then(|| rest.trim_start())
}

/// The identifier `s` starts with (empty when it doesn't start with one).
fn ident(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(s.len());
    &s[..end]
}

fn item(line: usize, kind: &'static str, name: &str) -> Option<Item> {
    (!name.is_empty()).then(|| Item {
        line,
        kind,
        name: name.to_string(),
    })
}

/// Top level means nothing before the definition on its line.
fn top_level(line: &str) -> bool {
    !line.is_empty() && !line.starts_with([' ', '\t'])
}

fn rust((n, line): (usize, &str)) -> Option<Item> {
    if !top_level(line) {
        return None;
    }
    let mut s = line;
    // `pub`, `pub(crate)`, `pub(in a::b)`.
    if let Some(rest) = s.strip_prefix("pub") {
        s = match rest.strip_prefix('(') {
            Some(r) => r[r.find(')')? + 1..].trim_start(),
            None => word(s, "pub")?,
        };
    }
    loop {
        if let Some(r) = ["async", "unsafe", "default"]
            .iter()
            .find_map(|w| word(s, w))
        {
            s = r;
        } else if let Some(r) = word(s, "extern") {
            // `extern "C" fn`; `extern crate` isn't a definition.
            s = match r.strip_prefix('"') {
                Some(abi) => abi[abi.find('"')? + 1..].trim_start(),
                None => r,
            };
        } else if let Some(r) = word(s, "const").filter(|r| {
            ["fn", "unsafe", "async", "extern"]
                .iter()
                .any(|w| word(r, w).is_some())
        }) {
            // `const fn`, as against `const NAME`.
            s = r;
        } else {
            break;
        }
    }
    if let Some(r) = s.strip_prefix("macro_rules!") {
        return item(n, "macro", ident(r.trim_start()));
    }
    // What's implemented, without the impl's own generics: `Display for Lane<T>`.
    let target = match s.strip_prefix("impl<") {
        Some(g) => Some(after_generics(g)?),
        None => word(s, "impl"),
    };
    if let Some(r) = target {
        let end = [r.find('{'), r.find(" where")]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(r.len());
        return item(n, "impl", r[..end].trim());
    }
    for kind in [
        "fn", "struct", "enum", "trait", "union", "mod", "type", "const", "static",
    ] {
        if let Some(r) = word(s, kind) {
            let r = word(r, "mut").unwrap_or(r);
            return item(n, kind, ident(r));
        }
    }
    None
}

/// What follows the `<...>` that `s` is inside, `s` starting just after the `<`.
fn after_generics(s: &str) -> Option<&str> {
    let mut depth = 1;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[i + 1..].trim_start());
                }
            }
            _ => {}
        }
    }
    None
}

fn typescript((n, line): (usize, &str)) -> Option<Item> {
    if !top_level(line) {
        return None;
    }
    // CommonJS: `module.exports = ...`, `exports.name = ...`.
    if line.starts_with("module.exports") {
        return item(n, "export", "module.exports");
    }
    if let Some(r) = line.strip_prefix("exports.") {
        return item(n, "export", ident(r));
    }
    let mut s = line;
    let mut default = false;
    if let Some(r) = word(s, "export") {
        s = r;
        if let Some(r) = word(s, "default") {
            s = r;
            default = true;
        }
    }
    while let Some(r) = ["declare", "abstract", "async"]
        .iter()
        .find_map(|w| word(s, w))
    {
        s = r;
    }
    // A declaration without a name is only worth listing as the default export.
    let named = |kind: &'static str, rest: &str| match ident(rest) {
        "" if default => item(n, kind, "default"),
        name => item(n, kind, name),
    };
    if let Some(r) = s.strip_prefix("function")
        && (r.is_empty() || r.starts_with([' ', '\t', '*', '(']))
    {
        // `function name`, `function* name`, and `function (` after `export default`.
        return named("function", r.trim_start_matches(['*', ' ', '\t']));
    }
    if let Some(r) = word(s, "const") {
        return match word(r, "enum") {
            Some(e) => named("enum", e),
            None => named("const", r),
        };
    }
    for kind in ["class", "interface", "type", "enum", "let", "var"] {
        if let Some(r) = word(s, kind) {
            return named(kind, r);
        }
    }
    if let Some(r) = word(s, "namespace").or_else(|| word(s, "module")) {
        // `declare module "name"` names a module with a string.
        let name = match r.strip_prefix(['"', '\'']) {
            Some(q) => &q[..q.find(['"', '\'']).unwrap_or(q.len())],
            None => ident(r),
        };
        return item(n, "namespace", name);
    }
    if default {
        // `export default { ... }`, `export default App;`
        return item(n, "export", "default");
    }
    None
}

fn python((n, line): (usize, &str)) -> Option<Item> {
    if !top_level(line) {
        return None;
    }
    let s = word(line, "async").unwrap_or(line);
    if let Some(r) = word(s, "def") {
        return item(n, "def", ident(r));
    }
    word(s, "class").and_then(|r| item(n, "class", ident(r)))
}

fn go<'a>(lines: impl Iterator<Item = (usize, &'a str)>) -> Vec<Item> {
    let mut items = Vec::new();
    // Inside `type (`, `var (` or `const (`: each name one level in is a definition.
    let mut group: Option<&'static str> = None;
    for (n, line) in lines {
        if let Some(kind) = group {
            let t = line.trim_start();
            if t.starts_with(')') {
                group = None;
            } else if one_level_in(line) {
                items.extend(item(n, kind, ident(t)));
            }
            continue;
        }
        if !top_level(line) {
            continue;
        }
        if let Some(r) = word(line, "func") {
            match r.strip_prefix('(') {
                // A method: `func (s *Server) Start()` is `Server.Start`.
                Some(recv) => {
                    let Some(close) = recv.find(')') else {
                        continue;
                    };
                    let ty = recv[..close]
                        .split_whitespace()
                        .last()
                        .unwrap_or("")
                        .trim_start_matches('*');
                    let ty = &ty[..ty.find('[').unwrap_or(ty.len())];
                    let name = ident(recv[close + 1..].trim_start());
                    if !name.is_empty() {
                        items.extend(item(n, "method", &format!("{ty}.{name}")));
                    }
                }
                None => items.extend(item(n, "func", ident(r))),
            }
            continue;
        }
        for kind in ["type", "var", "const"] {
            if let Some(r) = word(line, kind) {
                match r.strip_prefix('(') {
                    Some(inner) if !inner.contains(')') => group = Some(kind),
                    Some(_) => {}
                    None => items.extend(item(n, kind, ident(r))),
                }
                break;
            }
        }
    }
    items
}

/// One level in from the left margin, as gofmt indents: one tab (or up to four spaces).
fn one_level_in(line: &str) -> bool {
    let t = line.trim_start();
    let indent = &line[..line.len() - t.len()];
    let code = !t.is_empty() && !t.starts_with("//");
    code && (indent == "\t" || (!indent.contains('\t') && (1..=4).contains(&indent.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(language: Language, text: &str) -> Vec<(usize, &'static str, String)> {
        outline(language, text)
            .into_iter()
            .map(|i| (i.line, i.kind, i.name))
            .collect()
    }

    fn own(v: &[(usize, &'static str, &str)]) -> Vec<(usize, &'static str, String)> {
        v.iter().map(|(l, k, n)| (*l, *k, n.to_string())).collect()
    }

    #[test]
    fn languages_come_from_the_extension() {
        for (path, language) in [
            ("src/main.rs", Some(Language::Rust)),
            ("app/page.TSX", Some(Language::TypeScript)),
            ("index.mjs", Some(Language::TypeScript)),
            ("tool.py", Some(Language::Python)),
            ("cmd/main.go", Some(Language::Go)),
            ("README.md", None),
            ("Makefile", None),
        ] {
            assert_eq!(Language::of(Path::new(path)), language, "{path}");
        }
    }

    #[test]
    fn rust_finds_items_at_the_left_margin() {
        let text = [
            "\u{feff}//! Docs.",
            "use std::fmt;",
            "",
            "#[derive(Debug)]",
            "pub struct Lane<T> {",
            "    pub name: String,",
            "}",
            "pub(crate) enum Kind { A }",
            "pub const MAX: u32 = 1;",
            "static mut COUNT: u32 = 0;",
            "pub const unsafe fn fast() {}",
            "pub async fn serve() {",
            "    fn inner() {}",
            "}",
            "impl<T: Clone> fmt::Display for Lane<T> where T: Copy {",
            "    fn fmt(&self) {}",
            "}",
            "unsafe impl Send for Kind {}",
            "pub trait Tool: Send {}",
            "type Result<T> = std::result::Result<T, ()>;",
            "mod tests;",
            "extern crate alloc;",
            "extern \"C\" fn callback() {}",
            "macro_rules! check {",
            "}",
        ]
        .join("\n");
        assert_eq!(
            names(Language::Rust, &text),
            own(&[
                (5, "struct", "Lane"),
                (8, "enum", "Kind"),
                (9, "const", "MAX"),
                (10, "static", "COUNT"),
                (11, "fn", "fast"),
                (12, "fn", "serve"),
                (15, "impl", "fmt::Display for Lane<T>"),
                (18, "impl", "Send for Kind"),
                (19, "trait", "Tool"),
                (20, "type", "Result"),
                (21, "mod", "tests"),
                (23, "fn", "callback"),
                (24, "macro", "check"),
            ])
        );
    }

    #[test]
    fn typescript_and_javascript_find_exports_and_declarations() {
        let text = [
            "import x from 'y';",
            "export const app = 1;",
            "export default class Page extends Base {",
            "  render() {}",
            "}",
            "export async function load() {}",
            "function* ids() {}",
            "export interface Props {}",
            "export type { Hidden } from './x';",
            "type Id = string;",
            "export const enum Color { Red }",
            "declare module \"lanes\" {}",
            "let count = 0;",
            "export { app };",
            "const { a, b } = obj;",
            "module.exports = app;",
            "exports.helper = 1;",
            "export abstract class Shape {}",
            "export default function () {}",
            "functional();",
        ]
        // Windows line endings read the same.
        .join("\r\n");
        assert_eq!(
            names(Language::TypeScript, &text),
            own(&[
                (2, "const", "app"),
                (3, "class", "Page"),
                (6, "function", "load"),
                (7, "function", "ids"),
                (8, "interface", "Props"),
                (10, "type", "Id"),
                (11, "enum", "Color"),
                (12, "namespace", "lanes"),
                (13, "let", "count"),
                (16, "export", "module.exports"),
                (17, "export", "helper"),
                (18, "class", "Shape"),
                (19, "function", "default"),
            ])
        );
    }

    #[test]
    fn python_finds_defs_and_classes() {
        let text = [
            "import os",
            "",
            "@dataclass",
            "class Lane(Base):",
            "    def method(self):",
            "        pass",
            "",
            "async def main():",
            "    pass",
            "def helper(): pass",
            "if __name__ == \"__main__\":",
            "    main()",
        ]
        .join("\n");
        assert_eq!(
            names(Language::Python, &text),
            own(&[
                (4, "class", "Lane"),
                (8, "def", "main"),
                (10, "def", "helper")
            ])
        );
    }

    #[test]
    fn go_finds_funcs_methods_and_grouped_names() {
        let text = [
            "package main",
            "",
            "import (",
            "\t\"fmt\"",
            ")",
            "",
            "const (",
            "\t// Max is the most.",
            "\tMax = 10",
            "\tMin = 1",
            ")",
            "",
            "type (",
            "\tServer struct {",
            "\t\tname string",
            "\t}",
            "\tID string",
            ")",
            "",
            "type List[T any] struct{}",
            "var Version = \"1\"",
            "func main() {}",
            "func (s *Server) Start() error { return nil }",
            "func (l List[T]) Len() int { return 0 }",
        ]
        .join("\n");
        assert_eq!(
            names(Language::Go, &text),
            own(&[
                (9, "const", "Max"),
                (10, "const", "Min"),
                (14, "type", "Server"),
                (17, "type", "ID"),
                (20, "type", "List"),
                (21, "var", "Version"),
                (22, "func", "main"),
                (23, "method", "Server.Start"),
                (24, "method", "List.Len"),
            ])
        );
    }
}
