#[derive(Debug, Clone)]
pub struct MarkdownRenderOptions {
    pub force_bullets: bool,
    pub include_system_messages: bool,
    pub include_tools: bool,
}

impl Default for MarkdownRenderOptions {
    fn default() -> Self {
        Self {
            force_bullets: true,
            include_system_messages: false,
            include_tools: false,
        }
    }
}
