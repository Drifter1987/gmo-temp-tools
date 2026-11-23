use rand::Rng;

pub fn now_ms() -> f64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_millis() as f64,
        Err(_) => 0.0,
    }
}

pub fn rand_hex(n: usize) -> String {
    let mut rng = rand::thread_rng();
    let mut out = String::with_capacity(n);
    const HEX: &[u8] = b"0123456789abcdef";
    while out.len() < n {
        out.push(HEX[rng.gen_range(0..16)] as char);
    }
    out
}

pub fn domain_of(address: &str) -> &str {
    address.rsplit_once('@').map(|(_, domain)| domain).unwrap_or("")
}

pub fn strip_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let flat = out.split_whitespace().collect::<Vec<_>>().join(" ");
    decode_entities(&flat)
}

fn decode_entities(input: &str) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// Third-party HTML is rendered in a sandboxed frame; strip the obvious active
/// parts before it leaves this process.
pub fn sanitize_html(input: &str) -> String {
    let mut text = input.to_string();
    loop {
        let lower = text.to_lowercase();
        let Some(start) = lower.find("<script") else { break };
        let Some(end_rel) = lower[start..].find("</script>") else {
            text.truncate(start);
            break;
        };
        let end = start + end_rel + "</script>".len();
        text.replace_range(start..end, "");
    }
    // Drop inline event handlers (onclick=, onload= ...).
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            let rest = &text[j..];
            let lower = rest.to_ascii_lowercase();
            if lower.starts_with("on") && lower[2..].chars().take(12).any(|c| c == '=') {
                // skip to the closing quote of the attribute
                if let Some(eq) = lower.find('=') {
                    let after = &rest[eq + 1..];
                    if let Some(quote) = after.chars().next() {
                        if quote == '"' || quote == '\'' {
                            if let Some(close) = after[1..].find(quote) {
                                i = j + eq + 1 + 1 + close + 1;
                                continue;
                            }
                        }
                    }
                }
            }
            out.push(' ');
            i = j;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

pub fn extract_code(text: &str) -> Option<String> {
    const KEYWORDS: [&str; 7] = [
        "code", "otp", "passcode", "verification", "verify", "pin", "security",
    ];
    let bytes = text.as_bytes();
    let mut best: Option<(i32, String)> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let len = i - start;
        if !(4..=8).contains(&len) {
            continue;
        }
        let code = &text[start..i];
        let w_start = start.saturating_sub(60);
        let w_end = (i + 60).min(text.len());
        let window = text[w_start..w_end].to_lowercase();
        let mut score = 1;
        for kw in KEYWORDS {
            if window.contains(kw) {
                score += 3;
            }
        }
        if text.len() < 160 {
            score += 1;
        }
        if best.as_ref().map(|b| score > b.0).unwrap_or(true) {
            best = Some((score, code.to_string()));
        }
    }
    best.map(|b| b.1)
}

pub fn extract_link(text: &str) -> Option<String> {
    for word in text.split_whitespace() {
        let candidate = word.trim_matches(|c: char| {
            !c.is_ascii_alphanumeric() && c != ':' && c != '/' && c != '.' && c != '-' && c != '_'
        });
        if candidate.starts_with("http://") || candidate.starts_with("https://") {
            if candidate.len() > 12 {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

pub fn find_link_in_html(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    for scheme in ["https://", "http://"] {
        let mut from = 0;
        while let Some(idx) = lower[from..].find(scheme) {
            let start = from + idx;
            let rest = &html[start..];
            let end = rest
                .find(|c: char| c == '"' || c == '\'' || c == '<' || c == ' ' || c == ')')
                .unwrap_or(rest.len());
            let link = &rest[..end];
            if link.len() > 20 {
                return Some(link.to_string());
            }
            from = start + scheme.len();
        }
    }
    None
}

pub fn preview(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max).collect();
    out.push('…');
    out
}

/// "2026-11-12T21:12:58+00:00" -> epoch milliseconds.
pub fn parse_iso_ms(input: &str) -> f64 {
    let bytes = input.as_bytes();
    if bytes.len() < 19 {
        return now_ms();
    }
    let num = |at: usize, len: usize| -> f64 {
        input
            .get(at..at + len)
            .and_then(|slice| slice.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let (year, month, day) = (num(0, 4), num(5, 2), num(8, 2));
    let (hour, minute, second) = (num(11, 2), num(14, 2), num(17, 2));
    let days = days_from_civil(year as i64, month as i64, day as i64) as f64;
    // Skip fractional seconds before looking for a numeric UTC offset. Several
    // providers return microseconds here, while mail.tm returns whole seconds.
    let mut offset_at = 19;
    let mut fraction_ms = 0.0;
    if bytes.get(offset_at) == Some(&b'.') {
        let fraction_start = offset_at + 1;
        offset_at += 1;
        while offset_at < bytes.len() && bytes[offset_at].is_ascii_digit() {
            offset_at += 1;
        }
        if offset_at > fraction_start {
            let fraction = input[fraction_start..offset_at].parse::<f64>().unwrap_or(0.0);
            let digits = (offset_at - fraction_start) as i32;
            fraction_ms = fraction / 10f64.powi(digits) * 1000.0;
        }
    }
    let mut offset_min = 0.0;
    if offset_at < bytes.len() {
        let sign = if bytes[offset_at] == b'-' { -1.0 } else { 1.0 };
        if bytes[offset_at] == b'+' || bytes[offset_at] == b'-' {
            let hours = num(offset_at + 1, 2);
            let minutes = if bytes.get(offset_at + 3) == Some(&b':') {
                num(offset_at + 4, 2)
            } else {
                num(offset_at + 3, 2)
            };
            offset_min = sign * (hours * 60.0 + minutes);
        }
    }
    days * 86_400_000.0
        + (hour * 3600.0 + minute * 60.0 + second) * 1000.0
        + fraction_ms
        - offset_min * 60_000.0
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iso_timestamps() {
        // 2023-11-14T22:13:20Z
        let ms = parse_iso_ms("2023-11-14T22:13:20+00:00");
        assert!((ms - 1_700_000_000_000.0).abs() < 1.0, "got {ms}");
        let offset = parse_iso_ms("2023-11-15T00:13:20+02:00");
        assert!((offset - 1_700_000_000_000.0).abs() < 1.0, "got {offset}");
        let fractional = parse_iso_ms("2023-11-14T22:13:20.123456+00:00");
        assert!((fractional - 1_700_000_000_123.0).abs() < 1.0, "got {fractional}");
    }

    #[test]
    fn pulls_codes_and_links() {
        assert_eq!(
            extract_code("Your verification code is 481920.").as_deref(),
            Some("481920")
        );
        assert_eq!(
            extract_link("Open https://acme.example/verify?t=1 now").as_deref(),
            Some("https://acme.example/verify?t=1")
        );
    }

    #[test]
    fn strips_script_and_handlers() {
        let dirty = r#"<p onclick="steal()">hi</p><script>alert(1)</script>"#;
        let clean = sanitize_html(dirty);
        assert!(!clean.contains("script"));
        assert!(!clean.contains("onclick"));
        assert!(clean.contains("hi"));
    }
}
