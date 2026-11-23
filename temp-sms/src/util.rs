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

pub fn digits_only(input: &str) -> String {
    input.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Normalise a phone number to the E.164 shape this app stores. The public
/// page publishes digits, carriers publish a leading `+`; both become one
/// canonical value. Rejecting impossible lengths avoids creating a card for a
/// truncated page link or a malformed webhook payload.
pub fn normalize_e164(input: &str) -> Option<String> {
    let digits = digits_only(input);
    if !(7..=15).contains(&digits.len()) {
        return None;
    }
    if digits.starts_with('0') {
        return None;
    }
    Some(format!("+{digits}"))
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

pub fn preview(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::normalize_e164;

    #[test]
    fn accepts_and_rejects_e164_lengths_and_prefixes() {
        assert_eq!(normalize_e164("447700900123").as_deref(), Some("+447700900123"));
        assert_eq!(normalize_e164("+1 (555) 010-0200").as_deref(), Some("+15550100200"));
        assert_eq!(normalize_e164("123456"), None);
        assert_eq!(normalize_e164("1234567890123456"), None);
        assert_eq!(normalize_e164("05550100200"), None);
    }
}


