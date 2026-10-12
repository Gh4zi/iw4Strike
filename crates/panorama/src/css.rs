//! Panorama stylesheets: rules of selectors and declarations, `@define` constants substituted,
//! `@import`s reported, `@keyframes` and other at-rules skipped.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combinator {
    /// `a b`: any ancestor.
    Descendant,
    /// `a > b`: the parent.
    Child,
}

/// One compound selector: `Label.title#Name:hover`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Compound {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub pseudos: Vec<String>,
    /// `:not( … )`: compounds the panel must not match.
    pub nots: Vec<Compound>,
}

/// A complex selector: compounds from the outermost to the subject, each joined to the next by
/// its combinator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub parts: Vec<(Compound, Combinator)>,
}

impl Selector {
    /// The compound the selector applies to.
    #[must_use]
    pub fn subject(&self) -> Option<&Compound> {
        self.parts.last().map(|(c, _)| c)
    }

    /// (ids, classes and pseudo-classes, types).
    #[must_use]
    pub fn specificity(&self) -> (u32, u32, u32) {
        self.parts.iter().fold((0, 0, 0), |(a, b, c), (part, _)| {
            let nots = part.nots.iter().fold((0, 0, 0), |(x, y, z), n| {
                (
                    x + u32::from(n.id.is_some()),
                    y + (n.classes.len() + n.pseudos.len()) as u32,
                    z + u32::from(n.tag.is_some()),
                )
            });
            (
                a + u32::from(part.id.is_some()) + nots.0,
                b + (part.classes.len() + part.pseudos.len()) as u32 + nots.1,
                c + u32::from(part.tag.is_some()) + nots.2,
            )
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub selector: Selector,
    pub specificity: (u32, u32, u32),
    /// Position in the cascade: later wins among equal specificity.
    pub order: usize,
    pub declarations: Vec<(String, String)>,
}

/// What a selector tests a panel by.
pub trait Element {
    fn tag(&self, panel: usize) -> &str;
    fn id(&self, panel: usize) -> Option<&str>;
    fn has_class(&self, panel: usize, class: &str) -> bool;
    fn has_pseudo(&self, panel: usize, pseudo: &str) -> bool;
    fn parent(&self, panel: usize) -> Option<usize>;
}

fn compound_matches(compound: &Compound, doc: &impl Element, panel: usize) -> bool {
    if let Some(tag) = &compound.tag
        && !tag.eq_ignore_ascii_case(doc.tag(panel))
    {
        return false;
    }
    if let Some(id) = &compound.id
        && !doc.id(panel).is_some_and(|own| own.eq_ignore_ascii_case(id))
    {
        return false;
    }
    compound.classes.iter().all(|c| doc.has_class(panel, c))
        && compound.pseudos.iter().all(|p| doc.has_pseudo(panel, p))
        && !compound.nots.iter().any(|n| compound_matches(n, doc, panel))
}

/// Whether `selector` applies to `panel`.
pub fn matches(selector: &Selector, doc: &impl Element, panel: usize) -> bool {
    fn from(parts: &[(Compound, Combinator)], doc: &impl Element, panel: usize) -> bool {
        let Some(((compound, _), outer)) = parts.split_last() else {
            return true;
        };
        if !compound_matches(compound, doc, panel) {
            return false;
        }
        let Some((_, combinator)) = outer.last() else {
            return true;
        };
        match combinator {
            Combinator::Child => doc.parent(panel).is_some_and(|p| from(outer, doc, p)),
            Combinator::Descendant => {
                let mut ancestor = doc.parent(panel);
                while let Some(a) = ancestor {
                    if from(outer, doc, a) {
                        return true;
                    }
                    ancestor = doc.parent(a);
                }
                false
            }
        }
    }
    from(&selector.parts, doc, panel)
}

fn parse_compound(text: &str) -> Option<Compound> {
    let mut compound = Compound::default();
    let mut chars = text.char_indices().peekable();
    let take = |chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>| {
        let mut out = String::new();
        while let Some(&(_, c)) = chars.peek() {
            if c.is_alphanumeric() || matches!(c, '-' | '_') {
                out.push(c);
                chars.next();
            } else {
                break;
            }
        }
        out
    };
    while let Some(&(_, c)) = chars.peek() {
        match c {
            '.' => {
                chars.next();
                compound.classes.push(take(&mut chars));
            }
            '#' => {
                chars.next();
                compound.id = Some(take(&mut chars));
            }
            ':' => {
                chars.next();
                let name = take(&mut chars).to_ascii_lowercase();
                if name == "not" && chars.peek().is_some_and(|&(_, c)| c == '(') {
                    chars.next();
                    let mut inner = String::new();
                    for (_, c) in chars.by_ref() {
                        if c == ')' {
                            break;
                        }
                        inner.push(c);
                    }
                    compound.nots.push(parse_compound(inner.trim())?);
                } else {
                    compound.pseudos.push(name);
                }
            }
            '*' => {
                chars.next();
            }
            c if c.is_alphanumeric() || c == '_' => compound.tag = Some(take(&mut chars)),
            _ => return None,
        }
    }
    Some(compound)
}

fn parse_selector(text: &str) -> Option<Selector> {
    let spaced = text.replace('>', " > ");
    let mut parts = Vec::new();
    let mut combinator = Combinator::Descendant;
    for token in spaced.split_whitespace() {
        if token == ">" {
            combinator = Combinator::Child;
            continue;
        }
        if let Some((_, last)) = parts.last_mut() {
            *last = combinator;
        }
        parts.push((parse_compound(token)?, Combinator::Descendant));
        combinator = Combinator::Descendant;
    }
    (!parts.is_empty()).then_some(Selector { parts })
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start + 2..].find("*/").map_or("", |end| &rest[start + 2 + end + 2..]);
    }
    out.push_str(rest);
    out
}

/// `value` with every `@define` constant it names replaced (whole identifiers only).
fn substitute(value: &str, defines: &HashMap<String, String>) -> String {
    if defines.is_empty() {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if let Some(v) = defines.get(word.as_str()) {
            out.push_str(v);
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in value.chars() {
        if c.is_alphanumeric() || matches!(c, '-' | '_') {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// The end of the block opening at `open` (the index of its `{`): the index after its `}`.
fn block_end(text: &str, open: usize) -> usize {
    let mut depth = 0;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return open + i + 1;
                }
            }
            _ => {}
        }
    }
    text.len()
}

/// A parsed stylesheet: its rules (orders from `first_order`) and the files it imports.
pub struct Parsed {
    pub rules: Vec<Rule>,
    pub imports: Vec<String>,
}

/// Parses `text`, numbering its rules from `first_order`.
pub fn parse(text: &str, first_order: usize) -> Parsed {
    let text = strip_comments(text);
    let mut defines: HashMap<String, String> = HashMap::new();
    let mut rules = Vec::new();
    let mut imports = Vec::new();
    let mut i = 0;
    let mut order = first_order;
    while i < text.len() {
        let rest = &text[i..];
        let trimmed = rest.trim_start();
        i += rest.len() - trimmed.len();
        if trimmed.is_empty() {
            break;
        }
        if let Some(at) = trimmed.strip_prefix('@') {
            let semi = at.find(';');
            let brace = at.find('{');
            match (semi, brace) {
                (Some(s), b) if b.is_none_or(|b| s < b) => {
                    let statement = &at[..s];
                    if let Some(define) = statement.strip_prefix("define") {
                        if let Some((name, value)) = define.split_once(':') {
                            let value = substitute(value.trim(), &defines);
                            defines.insert(name.trim().to_owned(), value);
                        }
                    } else if let Some(import) = statement.strip_prefix("import") {
                        let url = import.trim().trim_start_matches("url(").trim_end_matches(')');
                        imports.push(url.trim().trim_matches(['"', '\'']).to_owned());
                    }
                    i += 1 + s + 1;
                }
                (_, Some(b)) => i = block_end(&text, i + 1 + b),
                _ => break,
            }
            continue;
        }
        let Some(open) = trimmed.find('{') else {
            break;
        };
        let end = block_end(&text, i + open);
        let selectors = &trimmed[..open];
        let body = &text[i + open + 1..end.saturating_sub(1)];
        let declarations: Vec<(String, String)> = body
            .split(';')
            .filter_map(|d| d.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), substitute(v.trim(), &defines)))
            .filter(|(k, v)| !k.is_empty() && !v.is_empty())
            .collect();
        for selector in selectors.split(',').filter_map(|s| parse_selector(s.trim())) {
            rules.push(Rule {
                specificity: selector.specificity(),
                selector,
                order,
                declarations: declarations.clone(),
            });
            order += 1;
        }
        i = end;
    }
    Parsed { rules, imports }
}

/// A file's declarations from an inline `style="a: b; c: d;"` attribute.
pub fn parse_inline(text: &str) -> Vec<(String, String)> {
    text.split(';')
        .filter_map(|d| d.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree;
    impl Element for Tree {
        fn tag(&self, panel: usize) -> &str {
            ["CSGOHudHealthArmor", "Panel", "Label"][panel]
        }
        fn id(&self, _: usize) -> Option<&str> {
            None
        }
        fn has_class(&self, panel: usize, class: &str) -> bool {
            match panel {
                0 => class == "hud-HA" || class == "hud-HA--critical",
                1 => class == "hud-HA-health",
                _ => class == "hud-HA-health-label",
            }
        }
        fn has_pseudo(&self, _: usize, _: &str) -> bool {
            false
        }
        fn parent(&self, panel: usize) -> Option<usize> {
            panel.checked_sub(1)
        }
    }

    #[test]
    fn parses_defines_rules_and_matches_descendants() {
        let parsed = parse(
            "@define yBar : 27px;\n@keyframes 'x' { from { opacity: 0; } to { opacity: 1; } }\n\
             .hud-HA-bar { y: yBar; }\n/* c */.hud-HA--critical .hud-HA-health-label, Label { color: red; }",
            0,
        );
        assert_eq!(parsed.rules.len(), 3);
        assert_eq!(parsed.rules[0].declarations, vec![("y".into(), "27px".into())]);
        assert!(matches(&parsed.rules[1].selector, &Tree, 2));
        assert!(!matches(&parsed.rules[1].selector, &Tree, 1));
        assert_eq!(parsed.rules[1].specificity, (0, 2, 0));
        assert!(matches(&parsed.rules[2].selector, &Tree, 2));
        let child = parse("CSGOHudHealthArmor > Label { x: 1px; }", 0);
        assert!(!matches(&child.rules[0].selector, &Tree, 2));
        let not = parse(
            ".hud-HA:not(.hud-HA--critical) Label { x: 1px; } Label:not(.x) { y: 1px; }",
            0,
        );
        assert!(!matches(&not.rules[0].selector, &Tree, 2));
        assert!(matches(&not.rules[1].selector, &Tree, 2));
        assert_eq!(not.rules[0].specificity, (0, 2, 1));
    }
}
