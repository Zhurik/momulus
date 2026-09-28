//! Маскирование секретов во всём, что попадает в логи и в комментарии PR.

/// Чем заменяем секрет.
pub const MASK: &str = "***";

/// Набор строк, которые нельзя показывать.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Добавляет секрет; слишком короткие строки игнорируются, чтобы не
    /// превратить вывод в сплошные звёздочки.
    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        self.add(secret);
        self
    }

    pub fn add(&mut self, secret: impl Into<String>) {
        let secret = secret.into();
        if secret.trim().len() >= 6 && !self.secrets.contains(&secret) {
            self.secrets.push(secret);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    /// Заменяет известные секреты и логин-часть URL на [`MASK`].
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), MASK);
            }
        }
        redact_url_credentials(&out)
    }
}

/// Убирает `user:password@` из любых URL в тексте.
fn redact_url_credentials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(scheme_at) = find_scheme(rest) {
        let (before, tail) = rest.split_at(scheme_at.0);
        out.push_str(before);
        let scheme_len = scheme_at.1;
        let (scheme, after_scheme) = tail.split_at(scheme_len);
        out.push_str(scheme);

        let authority_end = after_scheme
            .find(|c: char| c == '/' || c == '?' || c == '#' || c.is_whitespace())
            .unwrap_or(after_scheme.len());
        let authority = &after_scheme[..authority_end];

        match authority.rsplit_once('@') {
            Some((_creds, host)) => {
                out.push_str(MASK);
                out.push('@');
                out.push_str(host);
            }
            None => out.push_str(authority),
        }
        rest = &after_scheme[authority_end..];
    }

    out.push_str(rest);
    out
}

/// Находит `(offset, len)` ближайшего `scheme://`.
fn find_scheme(text: &str) -> Option<(usize, usize)> {
    let at = text.find("://")?;
    let start = text[..at]
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.'))
        .map(|i| i + 1)
        .unwrap_or(0);
    if start == at {
        // Нет имени схемы — пропускаем эти три символа.
        return Some((at, 3));
    }
    Some((start, at - start + 3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_known_secrets() {
        let r = Redactor::new().with_secret("ghs_supersecrettoken");
        assert_eq!(
            r.redact("Authorization: Bearer ghs_supersecrettoken"),
            "Authorization: Bearer ***"
        );
    }

    #[test]
    fn masks_secret_in_the_middle_of_a_line() {
        let r = Redactor::new().with_secret("sk-ant-0123456789");
        let out = r.redact("pi failed: key sk-ant-0123456789 rejected");
        assert!(!out.contains("sk-ant-0123456789"), "{out}");
        assert!(out.contains("key *** rejected"), "{out}");
    }

    #[test]
    fn masks_credentials_in_remote_urls() {
        let r = Redactor::new();
        assert_eq!(
            r.redact(
                "fatal: could not read https://x-access-token:ghs_abc@github.com/acme/blog.git"
            ),
            "fatal: could not read https://***@github.com/acme/blog.git"
        );
    }

    #[test]
    fn keeps_plain_urls_intact() {
        let r = Redactor::new();
        let text = "see https://github.com/acme/blog/pull/1 for details";
        assert_eq!(r.redact(text), text);
    }

    #[test]
    fn handles_several_urls_in_one_line() {
        let r = Redactor::new();
        let out = r.redact("a https://u:p@host/one b https://host/two c");
        assert_eq!(out, "a https://***@host/one b https://host/two c");
    }

    #[test]
    fn ignores_short_secrets() {
        let r = Redactor::new().with_secret("abc");
        assert_eq!(r.redact("abc def"), "abc def");
        assert!(r.is_empty());
    }

    #[test]
    fn deduplicates_secrets() {
        let r = Redactor::new()
            .with_secret("ghs_supersecrettoken")
            .with_secret("ghs_supersecrettoken");
        assert_eq!(r.redact("ghs_supersecrettoken"), MASK);
    }

    #[test]
    fn survives_text_without_scheme() {
        let r = Redactor::new();
        assert_eq!(r.redact("just :// weird"), "just :// weird");
    }
}
