use crate::export::markdown::options::MarkdownRenderOptions;
use crate::models::{ContentBlock, Conversation, Message};
use chrono::{TimeZone, Utc};
use std::fmt::Write;

pub struct MarkdownRenderer<'a> {
    options: &'a MarkdownRenderOptions,
}

impl<'a> MarkdownRenderer<'a> {
    pub fn new(options: &'a MarkdownRenderOptions) -> Self {
        Self { options }
    }

    pub fn render(
        &self,
        conversation: &Conversation,
        messages: &[(Message, Vec<ContentBlock>)],
    ) -> String {
        let mut out = String::new();
        out.push_str(&self.render_conversation_header(conversation, None));

        for (message, blocks) in messages {
            if let Some(msg_rendered) = self.render_message(message, blocks) {
                out.push_str(&msg_rendered);
            }
        }
        out
    }

    pub fn render_conversation_header(
        &self,
        conversation: &Conversation,
        part_info: Option<(u32, u32)>,
    ) -> String {
        let mut out = String::new();
        let title = conversation
            .title
            .as_deref()
            .unwrap_or("Conversation sans titre");
        if let Some((part, total)) = part_info {
            let _ = writeln!(out, "# {} (Part {}/{})", title, part, total);
        } else {
            let _ = writeln!(out, "# {}", title);
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "**UUID :** {}", conversation.id);

        if let Some(create_time) = conversation.create_time {
            if let Some(dt) = Utc.timestamp_opt(create_time as i64, 0).single() {
                let _ = writeln!(out, "**Création :** {}", dt.format("%Y-%m-%dT%H:%M:%SZ"));
            }
        }
        let _ = writeln!(out);
        out
    }

    pub fn render_message_envelope(&self, message: &Message) -> Option<String> {
        let role = message.role.as_deref().unwrap_or("unknown").to_lowercase();
        if role == "system" && !self.options.include_system_messages {
            return None;
        }
        if role == "tool" && !self.options.include_tools {
            return None;
        }

        let mut out = String::new();
        let _ = writeln!(out, "---");
        let _ = writeln!(out);

        let role_display = match role.as_str() {
            "user" => "👤 User",
            "assistant" => "🤖 Assistant",
            "system" => "⚙️ System",
            "tool" => "🛠️ Tool",
            _ => "❓ Inconnu",
        };

        if message.ic > 0 {
            let _ = writeln!(out, "## {} (IC {})", role_display, message.ic);
        } else {
            let _ = writeln!(out, "## {}", role_display);
        }

        if let Some(create_time) = message.create_time {
            if let Some(dt) = Utc.timestamp_opt(create_time as i64, 0).single() {
                let _ = writeln!(out, "*Date : {}*", dt.format("%Y-%m-%dT%H:%M:%SZ"));
            }
        }
        let _ = writeln!(out);
        Some(out)
    }

    pub fn render_message(&self, message: &Message, blocks: &[ContentBlock]) -> Option<String> {
        if !blocks.iter().any(|b| self.has_content(b)) {
            // Skip empty messages
            return None;
        }

        let mut out = self.render_message_envelope(message)?;

        for block in blocks {
            self.render_block(&mut out, block);
        }
        let _ = writeln!(out);

        Some(out)
    }

    pub fn has_content(&self, block: &ContentBlock) -> bool {
        match block.kind.as_str() {
            "text" | "code" | "paragraph" => block
                .text_content
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty()),
            _ => block.json_content.is_some() || block.text_content.is_some(),
        }
    }

    pub fn render_block_content(
        &self,
        block: &ContentBlock,
        byte_start: usize,
        byte_end: usize,
    ) -> Option<String> {
        let mut out = String::new();
        match block.kind.as_str() {
            "text" | "paragraph" => {
                if let Some(text) = &block.text_content {
                    if byte_start < text.len() {
                        let end = std::cmp::min(byte_end, text.len());
                        // Ensure we slice at char boundaries
                        let mut safe_start = byte_start;
                        while safe_start < text.len() && !text.is_char_boundary(safe_start) {
                            safe_start -= 1;
                        }
                        let mut safe_end = end;
                        while safe_end > 0 && !text.is_char_boundary(safe_end) {
                            safe_end += 1; // Wait, we should probably round down or up, but up to text.len()
                        }
                        if safe_end > text.len() {
                            safe_end = text.len();
                        }

                        let sliced_text = &text[safe_start..safe_end];
                        let mut rendered_text = sliced_text.to_string();
                        if self.options.force_bullets {
                            rendered_text = rendered_text.replace("•", "-");
                        }
                        let _ = writeln!(out, "{}", rendered_text);
                        let _ = writeln!(out);
                    }
                }
            }
            "code" => {
                if let Some(text) = &block.text_content {
                    // For code blocks we don't slice currently to avoid breaking markdown, or we slice text only
                    let end = std::cmp::min(byte_end, text.len());
                    let mut safe_start = byte_start;
                    while safe_start < text.len() && !text.is_char_boundary(safe_start) {
                        safe_start -= 1;
                    }
                    let mut safe_end = end;
                    while safe_end < text.len() && !text.is_char_boundary(safe_end) {
                        safe_end += 1;
                    }
                    if safe_start == 0 {
                        let _ = writeln!(out, "```");
                    }
                    if safe_start < text.len() {
                        let _ = writeln!(out, "{}", &text[safe_start..safe_end]);
                    }
                    if safe_end == text.len() {
                        let _ = writeln!(out, "```");
                        let _ = writeln!(out);
                    }
                }
            }
            _ => {
                if let Some(json) = &block.json_content {
                    // Slicing JSON is risky, we usually just render it whole
                    let text = json.to_string();
                    let end = std::cmp::min(byte_end, text.len());
                    if byte_start == 0 {
                        let _ = writeln!(out, "```json");
                    }
                    let mut safe_start = byte_start;
                    while safe_start < text.len() && !text.is_char_boundary(safe_start) {
                        safe_start -= 1;
                    }
                    let mut safe_end = end;
                    while safe_end < text.len() && !text.is_char_boundary(safe_end) {
                        safe_end += 1;
                    }
                    if safe_start < text.len() {
                        let _ = writeln!(out, "{}", &text[safe_start..safe_end]);
                    }
                    if safe_end == text.len() {
                        let _ = writeln!(out, "```");
                        let _ = writeln!(out);
                    }
                } else if let Some(text) = &block.text_content {
                    let end = std::cmp::min(byte_end, text.len());
                    let mut safe_start = byte_start;
                    while safe_start < text.len() && !text.is_char_boundary(safe_start) {
                        safe_start -= 1;
                    }
                    let mut safe_end = end;
                    while safe_end < text.len() && !text.is_char_boundary(safe_end) {
                        safe_end += 1;
                    }
                    if safe_start < text.len() {
                        let _ = writeln!(out, "{}", &text[safe_start..safe_end]);
                        let _ = writeln!(out);
                    }
                }
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    pub fn render_block(&self, out: &mut String, block: &ContentBlock) {
        if let Some(rendered) = self.render_block_content(block, 0, usize::MAX) {
            out.push_str(&rendered);
        }
    }

    pub fn count_words(text: &str) -> u64 {
        text.split_whitespace().count() as u64
    }

    pub fn count_bytes(text: &str) -> u64 {
        text.len() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ContentBlock, Conversation, Message};

    #[test]
    fn test_render_simple_conversation() {
        let options = MarkdownRenderOptions::default();
        let renderer = MarkdownRenderer::new(&options);

        let conv = Conversation {
            id: "test-uuid".to_string(),
            title: Some("Test Title".to_string()),
            create_time: Some(1672531200.0), // 2023-01-01T00:00:00Z
            update_time: None,
            current_node_id: None,
            default_model_slug: None,
            is_archived: false,
            is_starred: false,
            source_relative_path: "path".to_string(),
        };

        let msg1 = Message {
            id: "msg1".to_string(),
            ic: 10,
            node_id: "node1".to_string(),
            conversation_id: "test-uuid".to_string(),
            role: Some("user".to_string()),
            author_name: None,
            create_time: Some(1672531205.0),
            timestamp: None,
            source_shard_index: 0,
            source_node_order: 1,
            model_slug: None,
            content_type: Some("text".to_string()),
            is_active: true,
        };

        let block1 = ContentBlock {
            id: "block1".to_string(),
            message_id: "msg1".to_string(),
            ordinal: 0,
            kind: "text".to_string(),
            text_content: Some("Hello bot!".to_string()),
            json_content: None,
        };

        let messages = vec![(msg1, vec![block1])];

        let out = renderer.render(&conv, &messages);

        assert!(out.contains("# Test Title\n"));
        assert!(out.contains("**UUID :** test-uuid\n"));
        assert!(out.contains("**Création :** 2023-01-01T00:00:00Z\n"));
        assert!(out.contains("## 👤 User (IC 10)\n"));
        assert!(out.contains("*Date : 2023-01-01T00:00:05Z*\n"));
        assert!(out.contains("Hello bot!\n"));
    }
}
