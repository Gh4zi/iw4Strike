//! The XML of Panorama layouts: elements, their attributes and child elements. Text, comments,
//! the declaration and doctypes are skipped; that is all `layout/*.xml` holds.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
}

impl Element {
    /// Attribute `key` (case-insensitive).
    #[must_use]
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// The first child element named `name` (case-insensitive).
    #[must_use]
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Parses `text` into a document whose children are its top-level elements.
pub fn parse(text: &str) -> Result<Element, String> {
    let bytes = text.as_bytes();
    let mut stack = vec![Element::default()];
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        if rest.starts_with("<!--") {
            i += rest.find("-->").map_or(rest.len(), |end| end + 3);
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            i += rest.find('>').map_or(rest.len(), |end| end + 1);
            continue;
        }
        if let Some(close) = rest.strip_prefix("</") {
            let end = close.find('>').ok_or("unterminated closing tag")?;
            let name = close[..end].trim();
            let element = stack.pop().ok_or("closing tag without an element")?;
            if !element.name.eq_ignore_ascii_case(name) {
                return Err(format!("</{name}> closes <{}>", element.name));
            }
            stack
                .last_mut()
                .ok_or("closing tag above the document")?
                .children
                .push(element);
            i += 2 + end + 1;
            continue;
        }
        // An opening tag: its name, then attributes up to `>` or `/>` (quotes may hold `>`).
        let mut j = i + 1;
        while j < bytes.len() && !bytes[j].is_ascii_whitespace() && !matches!(bytes[j], b'>' | b'/')
        {
            j += 1;
        }
        let mut element = Element {
            name: text[i + 1..j].to_owned(),
            ..Element::default()
        };
        let mut self_closing = false;
        loop {
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= bytes.len() {
                return Err(format!("unterminated <{}>", element.name));
            }
            if bytes[j] == b'>' {
                j += 1;
                break;
            }
            if bytes[j] == b'/' {
                self_closing = true;
                j += 1;
                continue;
            }
            let key_start = j;
            while j < bytes.len() && !matches!(bytes[j], b'=' | b'>' | b'/') && !bytes[j].is_ascii_whitespace()
            {
                j += 1;
            }
            let key = text[key_start..j].to_owned();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            let mut value = String::new();
            if j < bytes.len() && bytes[j] == b'=' {
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && matches!(bytes[j], b'"' | b'\'') {
                    let quote = bytes[j];
                    let start = j + 1;
                    j = start;
                    while j < bytes.len() && bytes[j] != quote {
                        j += 1;
                    }
                    value = unescape(&text[start..j.min(bytes.len())]);
                    j += 1;
                } else {
                    let start = j;
                    while j < bytes.len() && !bytes[j].is_ascii_whitespace() && bytes[j] != b'>' {
                        j += 1;
                    }
                    value = unescape(&text[start..j]);
                }
            }
            if !key.is_empty() {
                element.attrs.push((key, value));
            }
        }
        i = j;
        if self_closing {
            stack
                .last_mut()
                .ok_or("element above the document")?
                .children
                .push(element);
        } else {
            stack.push(element);
        }
    }
    // Tolerate unclosed elements at the end the way a lenient reader would.
    while stack.len() > 1 {
        let element = stack.pop().unwrap_or_default();
        if let Some(parent) = stack.last_mut() {
            parent.children.push(element);
        }
    }
    stack.pop().ok_or_else(|| "empty document".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_elements_attributes_and_skips_comments() {
        let doc = parse(
            "<root><!-- a <note> --><styles><include src=\"file://{resources}/a.css\" /></styles>\
             <Panel id='Top' class=\"a b\"><Label text=\"{s:health}\"/></Panel></root>",
        )
        .expect("parses");
        let root = doc.child("root").expect("root");
        assert_eq!(
            root.child("styles").and_then(|s| s.child("include")).and_then(|i| i.attr("src")),
            Some("file://{resources}/a.css")
        );
        let panel = root.child("Panel").expect("panel");
        assert_eq!(panel.attr("ID"), Some("Top"));
        assert_eq!(panel.children[0].attr("text"), Some("{s:health}"));
    }
}
