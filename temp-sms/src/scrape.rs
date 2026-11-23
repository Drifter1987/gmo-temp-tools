//! Reading the public pages of receive-sms.io.
//!
//! The site server-renders everything we need: country pages link to numbers,
//! and each number page carries the messages as `div.sms-item` blocks. Parsing
//! is done with plain string scanning so it stays testable without the network.

pub const BASE: &str = "https://receive-sms.io";
pub const AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

/// Country slugs linked from the home page, e.g. "australia".
pub fn countries(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    let needle = "/temporary-numbers/";
    while let Some(idx) = html[from..].find(needle) {
        let start = from + idx + needle.len();
        let rest = &html[start..];
        let end = rest
            .find(|c: char| c == '"' || c == '\'' || c == '<' || c == ' ')
            .unwrap_or(rest.len());
        let path = &rest[..end];
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        if parts.len() == 1 && parts[0].chars().all(|c| c.is_ascii_lowercase() || c == '-') {
            out.push(parts[0].to_string());
        }
        from = start;
    }
    out.sort();
    out.dedup();
    out
}

/// (country slug, digits) pairs linked from a country page.
pub fn numbers(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    let needle = "/temporary-numbers/";
    while let Some(idx) = html[from..].find(needle) {
        let start = from + idx + needle.len();
        let rest = &html[start..];
        let end = rest
            .find(|c: char| c == '"' || c == '\'' || c == '<' || c == ' ')
            .unwrap_or(rest.len());
        let path = &rest[..end];
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        if parts.len() == 2 && parts[1].len() >= 7 && parts[1].chars().all(|c| c.is_ascii_digit())
        {
            out.push((parts[0].to_string(), parts[1].to_string()));
        }
        from = start;
    }
    out.sort();
    out.dedup();
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScrapedMessage {
    pub sender: String,
    pub body: String,
    pub received_at: f64,
    pub when: String,
}

pub fn messages(html: &str) -> Vec<ScrapedMessage> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(idx) = html[from..].find("class=\"sms-item\"") {
        let block_start = from + idx;
        let rest = &html[block_start..];
        let block_end = rest.find("</div>\n\t\t\t\t\t\t\t<div class=\"sms-item\"")
            .or_else(|| rest[3..].find("class=\"sms-item\""))
            .unwrap_or(rest.len());
        let block = &rest[..block_end.min(rest.len())];

        let sender = inner_after(block, "class=\"left\"").unwrap_or_default();
        let when = inner_after(block, "class=\"right\"").unwrap_or_default();
        let body = inner_after(block, "class=\"center\"")
            .map(|raw| strip_tags(&raw))
            .unwrap_or_default();
        let seconds = attr_value(block, "data-time=\"")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);

        if !body.trim().is_empty() {
            out.push(ScrapedMessage {
                sender: repair_text(&strip_tags(&sender)),
                body: repair_text(&body),
                received_at: seconds * 1000.0,
                when: when.trim().to_string(),
            });
        }
        from = block_start + block.len().max(1);
    }
    out
}

/// Text between the `>` of the opening tag carrying `marker` and its `</div>`.
fn inner_after(block: &str, marker: &str) -> Option<String> {
    let at = block.find(marker)?;
    let rest = &block[at..];
    let open_end = rest.find('>')?;
    let content = &rest[open_end + 1..];
    let close = content.find("</div>").unwrap_or(content.len());
    Some(content[..close].trim().to_string())
}

fn attr_value(block: &str, prefix: &str) -> Option<String> {
    let at = block.find(prefix)?;
    let rest = &block[at + prefix.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// The source sometimes stores UTF-8 that was decoded as Latin-1 and saved
/// again, so "ğ" arrives as "ÄŸ". Undo that only when the whole string is
/// Latin-1 and its bytes form valid UTF-8; anything else is returned untouched.
pub fn repair_text(input: &str) -> String {
    let mut bytes = Vec::with_capacity(input.len());
    for c in input.chars() {
        match u8::try_from(c as u32) {
            Ok(byte) => bytes.push(byte),
            Err(_) => return input.to_string(),
        }
    }
    match String::from_utf8(bytes) {
        Ok(fixed) if fixed != input => fixed,
        _ => input.to_string(),
    }
}

pub fn strip_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Copied from a live number page, whitespace and all.
    const PAGE: &str = r#"
				<div class="sms-item" data-time="1794493859">
			<div class="left" data-title="From">PGbonus</div>
		
	<div class="right" data-title="Time">
		recently	</div>

	<div class="center" data-title="Message"><span dir="ltr">537047 is your PGbonus verification code. Lets get started!</span></div>
</div>
							<div class="sms-item" data-time="1794493823">
			<div class="left" data-title="From">SimplexCC</div>
		
	<div class="right" data-title="Time">
		recently	</div>

	<div class="center" data-title="Message"><span dir="ltr">SimplexCC code: 7009. Valid for 5 minutes.</span></div>
</div>
"#;

    #[test]
    fn reads_messages_from_a_number_page() {
        let found = messages(PAGE);
        assert_eq!(found.len(), 2, "got {found:?}");
        assert_eq!(found[0].sender, "PGbonus");
        assert_eq!(
            found[0].body,
            "537047 is your PGbonus verification code. Lets get started!"
        );
        assert_eq!(found[0].received_at, 1_794_493_859_000.0);
        assert_eq!(found[1].sender, "SimplexCC");
        assert!(found[1].body.contains("7009"));
    }

    #[test]
    fn extracts_codes_from_scraped_text() {
        let found = messages(PAGE);
        assert_eq!(crate::util::extract_code(&found[0].body).as_deref(), Some("537047"));
        assert_eq!(crate::util::extract_code(&found[1].body).as_deref(), Some("7009"));
    }

    #[test]
    fn repairs_double_encoded_text() {
        // "Doğrulama sayfasına" as the source actually serves it.
        assert_eq!(
            repair_text("Do\u{00c4}\u{009f}rulama sayfas\u{00c4}\u{00b1}na"),
            "Doğrulama sayfasına"
        );
        // Real Latin-1 text is left alone.
        assert_eq!(repair_text("café"), "café");
        assert_eq!(repair_text("plain ASCII"), "plain ASCII");
        // Already-correct non-ASCII is left alone.
        assert_eq!(repair_text("Código"), "Código");
    }

    #[test]
    fn lists_numbers_and_countries() {
        let country = r#"
          <a href="https://receive-sms.io/temporary-numbers/australia/61400652756/">+61400652756</a>
          <a href="/temporary-numbers/australia/61400652757/">+61400652757</a>
        "#;
        let found = numbers(country);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], ("australia".to_string(), "61400652756".to_string()));

        let home = r#"<a href="/temporary-numbers/australia/">Australia</a>
                      <a href="/temporary-numbers/germany/">Germany</a>"#;
        assert_eq!(countries(home), vec!["australia", "germany"]);
    }
}
