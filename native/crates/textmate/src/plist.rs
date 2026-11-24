//! The plist XML subset TextMate grammars use (`dict`, `key`, `string`, `array`, `integer`,
//! `true`, `false`), read into JSON values as vscode-textmate's plist.ts does.

use serde_json::{Map, Value};

pub fn parse(content: &str) -> Result<Value, String> {
    let mut parser = Parser { text: content, pos: 0 };
    let start = content.find("<plist").ok_or("no <plist> element")?;
    parser.pos = start;
    parser.skip_tag();
    parser.value()
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.text[self.pos..]
    }

    /// Skips whitespace and comments, then returns the next tag name without consuming it.
    fn peek_tag(&mut self) -> Result<String, String> {
        loop {
            let trimmed = self.rest().trim_start();
            self.pos = self.text.len() - trimmed.len();
            if trimmed.starts_with("<!--") {
                let end = trimmed.find("-->").ok_or("unterminated comment")?;
                self.pos += end + 3;
                continue;
            }
            if !trimmed.starts_with('<') {
                return Err(format!("expected a tag at {}", self.pos));
            }
            // From the second character on, so the `/` of a closing tag stays in its name.
            let end = trimmed.get(2..).and_then(|name| name.find(|c: char| c == '>' || c == '/' || c.is_whitespace())).ok_or("bad tag")? + 2;
            return Ok(trimmed[1..end].to_string());
        }
    }

    fn skip_tag(&mut self) {
        let end = self.rest().find('>').map_or(self.text.len(), |i| self.pos + i + 1);
        self.pos = end;
    }

    fn self_closing(&self) -> bool {
        let end = self.rest().find('>').unwrap_or(0);
        self.rest()[..end].ends_with('/')
    }

    fn text_until(&mut self, close: &str) -> Result<String, String> {
        let end = self.rest().find(close).ok_or_else(|| format!("missing {close}"))?;
        let raw = &self.rest()[..end];
        let text = unescape(raw);
        self.pos += end + close.len();
        Ok(text)
    }

    fn value(&mut self) -> Result<Value, String> {
        let tag = self.peek_tag()?;
        let empty = self.self_closing();
        self.skip_tag();
        Ok(match tag.as_str() {
            "dict" if empty => Value::Object(Map::new()),
            "dict" => {
                let mut map = Map::new();
                loop {
                    let tag = self.peek_tag()?;
                    if tag == "/dict" {
                        self.skip_tag();
                        break;
                    }
                    if tag != "key" {
                        return Err(format!("expected <key>, got <{tag}>"));
                    }
                    self.skip_tag();
                    let key = self.text_until("</key>")?;
                    map.insert(key, self.value()?);
                }
                Value::Object(map)
            }
            "array" if empty => Value::Array(Vec::new()),
            "array" => {
                let mut items = Vec::new();
                while self.peek_tag()? != "/array" {
                    items.push(self.value()?);
                }
                self.skip_tag();
                Value::Array(items)
            }
            "string" if empty => Value::String(String::new()),
            "string" => Value::String(self.text_until("</string>")?),
            "integer" => Value::from(self.text_until("</integer>")?.trim().parse::<i64>().map_err(|e| e.to_string())?),
            "real" => Value::from(self.text_until("</real>")?.trim().parse::<f64>().map_err(|e| e.to_string())?),
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            other => return Err(format!("unsupported plist tag <{other}>")),
        })
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else { break };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if entity.starts_with("#x") => u32::from_str_radix(&entity[2..], 16).ok().and_then(char::from_u32),
            _ if entity.starts_with('#') => entity[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_dicts_arrays_and_entities() {
        let value = super::parse(
            r#"<?xml version="1.0"?><!DOCTYPE plist><plist version="1.0"><dict>
            <key>name</key><string>a &lt;b&gt; &amp;</string><!-- c -->
            <key>list</key><array><true/><integer>3</integer><dict/></array>
            </dict></plist>"#,
        )
        .unwrap();
        assert_eq!(value, serde_json::json!({ "name": "a <b> &", "list": [true, 3, {}] }));
    }
}
