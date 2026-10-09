/// Replaces known secret values and any `api-key=<value>` query parameter with `<redacted>`.
#[derive(Clone, Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    pub fn new(secrets: Vec<String>) -> Self {
        Self { secrets }
    }

    pub fn redact(&self, s: &str) -> String {
        let mut out = s.to_string();
        for sec in &self.secrets {
            if out.contains(sec.as_str()) {
                out = out.replace(sec.as_str(), "<redacted>");
            }
        }
        redact_query_param(&out, "api-key=")
    }
}

fn redact_query_param(s: &str, key: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(key) {
        out.push_str(&rest[..i + key.len()]);
        rest = &rest[i + key.len()..];
        let end = rest
            .find(|c: char| c == '&' || c == '"' || c == '\'' || c == ' ' || c == '\n' || c == ')' || c == ',')
            .unwrap_or(rest.len());
        if !rest[..end].starts_with("<redacted>") && end > 0 {
            out.push_str("<redacted>");
        } else {
            out.push_str(&rest[..end]);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_values_and_query_params() {
        let r = Redactor::new(vec!["s3cr3t-key-value".into()]);
        assert_eq!(r.redact("token s3cr3t-key-value end"), "token <redacted> end");
        assert_eq!(
            r.redact("wss://h/?api-key=other-unknown-key&x=1"),
            "wss://h/?api-key=<redacted>&x=1"
        );
        assert_eq!(r.redact("url https://h/?api-key=s3cr3t-key-value"), "url https://h/?api-key=<redacted>");
        assert_eq!(r.redact("no secrets"), "no secrets");
    }
}
