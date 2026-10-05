//! Valve KeyValues text (VMT materials, sound scripts): quoted or bare tokens, `{ }` blocks,
//! `//` comments. Parsed into a flat list of `(key, value)` with nested blocks.

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    Block(Vec<(String, Value)>),
}

impl Value {
    /// First `key` (case-insensitive) in this block, if it is text.
    #[must_use]
    pub fn text(&self, key: &str) -> Option<&str> {
        self.entries()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .and_then(|(_, v)| match v {
                Value::Text(t) => Some(t.as_str()),
                Value::Block(_) => None,
            })
    }

    /// First `key` (case-insensitive) in this block, if it is a block.
    #[must_use]
    pub fn block(&self, key: &str) -> Option<&Value> {
        self.entries()
            .find(|(k, v)| k.eq_ignore_ascii_case(key) && matches!(v, Value::Block(_)))
            .map(|(_, v)| v)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&String, &Value)> {
        match self {
            Value::Block(items) => items
                .iter()
                .map(|(k, v)| (k, v))
                .collect::<Vec<_>>()
                .into_iter(),
            Value::Text(_) => Vec::new().into_iter(),
        }
    }
}

fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '/' {
            chars.next();
            if chars.peek() == Some(&'/') {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            } else {
                out.push("/".to_owned());
            }
        } else if c == '{' || c == '}' {
            out.push(c.to_string());
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut s = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                s.push(c);
            }
            out.push(s);
        } else {
            let mut s = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
                    break;
                }
                s.push(c);
                chars.next();
            }
            out.push(s);
        }
    }
    out
}

/// Parse a whole file: the top level is a block of `key value-or-block` pairs.
#[must_use]
pub fn parse(text: &str) -> Value {
    let tokens = tokens(text);
    let mut at = 0;
    Value::Block(parse_block(&tokens, &mut at))
}

fn parse_block(tokens: &[String], at: &mut usize) -> Vec<(String, Value)> {
    let mut items = Vec::new();
    while *at < tokens.len() {
        let key = &tokens[*at];
        *at += 1;
        if key == "}" {
            break;
        }
        if key == "{" {
            // An anonymous block: keep it under an empty key.
            items.push((String::new(), Value::Block(parse_block(tokens, at))));
            continue;
        }
        let Some(next) = tokens.get(*at) else {
            break;
        };
        if next == "{" {
            *at += 1;
            items.push((key.clone(), Value::Block(parse_block(tokens, at))));
        } else {
            *at += 1;
            items.push((key.clone(), Value::Text(next.clone())));
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_and_sound_script_shapes() {
        let vmt = parse(
            "\"VertexLitGeneric\"\n{\n  \"$basetexture\" \"models/weapons/v_models/rif_ak47/rif_ak47skin1\" // c\n  $alphatest 1\n}\n",
        );
        let shader = vmt.entries().next().expect("shader").1;
        assert_eq!(
            shader.text("$BaseTexture"),
            Some("models/weapons/v_models/rif_ak47/rif_ak47skin1")
        );
        assert_eq!(shader.text("$alphatest"), Some("1"));

        let sounds = parse(
            "\"Weapon_AK47.Single\" { \"channel\" \"CHAN_WEAPON\" \"rndwave\" { \"wave\" \"a.wav\" \"wave\" \"b.wav\" } }",
        );
        let entry = sounds.block("weapon_ak47.single").expect("entry");
        let waves: Vec<_> = entry
            .block("rndwave")
            .expect("rndwave")
            .entries()
            .map(|(_, v)| v.clone())
            .collect();
        assert_eq!(waves.len(), 2);
    }
}
