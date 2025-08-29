use crate::model::{ChatRequest, EmbedRequest};
use unicode_normalization::UnicodeNormalization;
use std::collections::HashSet;

fn clean_text(s: &str) -> String {
    // Unicode NFC normalization + BOM strip + CRLF -> LF + trim
    let mut t = s.nfc().collect::<String>();
    if t.starts_with('\u{FEFF}') { // Byte Order Mark
        t.remove(0);
    }
    if t.contains("\r\n") {
        t = t.replace("\r\n", "\n");
    }
    t.trim().to_string()
}

fn clamp_round_f32(x: f32, lo: f32, hi: f32, dp: u32) -> f32 {
    let clamped = x.clamp(lo, hi);
    let p = 10f32.powi(dp as i32);
    (clamped * p).round() / p
}

pub fn normalize_chat(mut req: ChatRequest) -> ChatRequest {
    for msg in &mut req.messages {
        msg.content = clean_text(&msg.content);
    }
    // Default and clamp numeric params
    req.temperature = Some(match req.temperature {
        Some(t) => clamp_round_f32(t, 0.0, 2.0, 3),
        None => 1.0,
    });
    req.top_p = Some(match req.top_p {
        Some(p) => clamp_round_f32(p, 0.0, 1.0, 4),
        None => 1.0,
    });
    if let Some(stops) = &mut req.stop_sequences {
        stops.sort();
        stops.dedup();
        if stops.is_empty() {
            req.stop_sequences = None;
        }
    }
    if let Some(max) = req.max_output_tokens {
        if max > 100_000 { req.max_output_tokens = Some(100_000); }
    }
    req
}

pub fn normalize_embed(mut req: EmbedRequest) -> EmbedRequest {
    req.inputs = req.inputs
        .into_iter()
        .map(|s| clean_text(&s))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    req.inputs.retain(|s| seen.insert(s.clone()));
    req
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChatMessage, Role};

    fn mk_chat_req(msgs: Vec<(&'static str, &'static str)>) -> ChatRequest {
        ChatRequest {
            model: "gpt-4o".to_string(),
            messages: msgs
                .into_iter()
                .map(|(role, content)| ChatMessage { role: match role {
                    "user" => Role::User,
                    "assistant" => Role::Assistant,
                    "system" => Role::System,
                    "tool" => Role::Tool,
                    _ => Role::User,
                }, content: content.to_string() })
                .collect(),
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: None,
            stop_sequences: None,
        }
    }

    #[test]
    fn trims_message_content_and_defaults_params() {
        let req = mk_chat_req(vec![("user", "  Hello world   ")]);
        let out = normalize_chat(req);
        assert_eq!(out.messages[0].content, "Hello world");
        assert_eq!(out.temperature, Some(1.0));
        assert_eq!(out.top_p, Some(1.0));
    }

    #[test]
    fn dedups_and_cleans_stop_sequences() {
        let mut req = mk_chat_req(vec![("user", "go")]);
        req.stop_sequences = Some(vec!["END".into(), "END".into(), "STOP".into()]);
        let out = normalize_chat(req);
        assert_eq!(out.stop_sequences.as_ref().unwrap().len(), 2);
        assert!(out.stop_sequences.as_ref().unwrap().contains(&"END".into()));
        assert!(out.stop_sequences.as_ref().unwrap().contains(&"STOP".into()));
    }

    #[test]
    fn empty_stop_sequences_become_none() {
        let mut req = mk_chat_req(vec![("user", "go")]);
        req.stop_sequences = Some(vec![]);
        let out = normalize_chat(req);
        assert!(out.stop_sequences.is_none());
    }

    #[test]
    fn caps_max_output_tokens() {
        let mut req = mk_chat_req(vec![("user", "go")]);
        req.max_output_tokens = Some(200_000);
        let out = normalize_chat(req);
        assert_eq!(out.max_output_tokens, Some(100_000));
    }

    #[test]
    fn normalize_embed_trims_and_drops_empty() {
        let req = EmbedRequest {
            model: "text-embedding-3-small".to_string(),
            inputs: vec!["  one  ".into(), "".into(), " two".into(), "three ".into()],
            client_key: None,
        };
        let out = normalize_embed(req);
        assert_eq!(out.inputs, vec!["one", "two", "three"]);
    }

    #[test]
    fn unicode_nfc_and_crlf_normalization() {
        // "e" + combining acute accent should normalize to "é"
        let req = mk_chat_req(vec![("user", "e\u{301}")]);
        let out = normalize_chat(req);
        assert_eq!(out.messages[0].content, "é");

        // CRLF becomes LF
        let req2 = mk_chat_req(vec![("user", "line1\r\nline2")]);
        let out2 = normalize_chat(req2);
        assert_eq!(out2.messages[0].content, "line1\nline2");
    }

    #[test]
    fn dedup_embedding_inputs_after_clean() {
        let req = EmbedRequest {
            model: "text-embedding-3-small".to_string(),
            inputs: vec![" a ".into(), "a".into(), "a".into(), "b".into(), " b".into()],
            client_key: None,
        };
        let out = normalize_embed(req);
        assert_eq!(out.inputs, vec!["a", "b"]);
    }

    #[test]
    fn clamp_and_round_floats() {
        let mut req = mk_chat_req(vec![("user", "go")]);
        req.temperature = Some(2.0000002);
        req.top_p = Some(1.0000001);
        let out = normalize_chat(req);
        assert_eq!(out.temperature, Some(2.0));
        assert_eq!(out.top_p, Some(1.0));
    }
}