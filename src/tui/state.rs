use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use arboard::Clipboard;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::ListState;
use ratatui_textarea::TextArea;

use crate::{
    read_model::{
        AssetView, BranchChoice, ConversationListItem, MessageView, ReadRepository, SearchHit,
    },
    Result,
};

use super::render::build_reader_lines;
use super::text::{truncate, wrap_content};

pub const PAGE_SIZE: usize = 150;
pub const ALL_MESSAGES_CAP: usize = 500;
pub const THREAD_CACHE_LIMIT: usize = 12;
pub const CONVERSATION_LOAD_DEBOUNCE: Duration = Duration::from_millis(180);

#[derive(Clone)]
pub struct ThreadCacheEntry {
    messages: Vec<MessageView>,
}

#[derive(Clone)]
pub struct ReaderRenderCache {
    wrap_width: u16,
    technical_visible: bool,
    messages_generation: u64,
    message_selected: usize,
    branch_alternatives: Option<usize>,
    lines: Vec<Line<'static>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReaderMode {
    Thread,
    AllMessages,
}

impl ReaderMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Thread => "Thread",
            Self::AllMessages => "All messages · IC order",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Conversations,
    Reader,
    Inspector,
    Input,
}

#[derive(Debug)]
pub enum Overlay {
    Branches {
        choices: Vec<BranchChoice>,
        selected: usize,
    },
    Context {
        lines: Vec<String>,
    },
    Help,
    SearchResults {
        query: String,
        hits: Vec<SearchHit>,
        selected: usize,
    },
}
impl Focus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Conversations => "Conversations",
            Self::Reader => "Reader",
            Self::Inspector => "Inspector",
            Self::Input => "Input",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    Search,
    Jump,
}

pub struct App {
    pub repository: ReadRepository,
    pub conversations: Vec<ConversationListItem>,
    pub conversations_state: ListState,
    pub conversations_offset: usize,
    pub conversations_exhausted: bool,
    pub messages: Vec<MessageView>,
    pub message_selected: usize,
    pub reader_scroll: u16,
    pub reader_wrap_width: u16,
    pub conversation_id: Option<String>,
    pub conversation_title: String,
    pub reader_mode: ReaderMode,
    pub focus: Focus,
    pub overlay: Option<Overlay>,
    pub technical_visible: bool,
    pub total_conversations: i64,
    pub conversation_message_count: i64,
    pub messages_capped: bool,
    pub branch_alternatives: Option<usize>,
    pub branch_hint_node: Option<String>,
    pub status: String,
    pub assets: Vec<AssetView>,
    pub should_quit: bool,
    pub clipboard: Option<Clipboard>,
    pub messages_generation: u64,
    pub line_count_cache: Vec<usize>,
    pub line_count_cache_width: u16,
    pub line_count_cache_technical: bool,
    pub line_count_cache_generation: u64,
    pub reader_render_cache: Option<ReaderRenderCache>,
    pub thread_cache: HashMap<String, ThreadCacheEntry>,
    pub thread_cache_order: Vec<String>,
    pub pending_conversation_id: Option<String>,
    pub conversation_nav_at: Instant,
    pub loading_conversation: bool,
    pub search_input: TextArea<'static>,
    pub input_mode: InputMode,
    pub conversations_rect: Rect,
    pub reader_rect: Rect,
    pub inspector_rect: Rect,
    pub input_rect: Rect,
}

impl App {
    pub fn new(repository: ReadRepository) -> Result<Self> {
        let conversations = repository.list_conversations("", PAGE_SIZE)?;
        let conversations_exhausted = conversations.len() < PAGE_SIZE;
        let total_conversations = repository.count_active_conversations()?;
        let mut conversations_state = ListState::default();
        if !conversations.is_empty() {
            conversations_state.select(Some(0));
        }
        let mut app = Self {
            repository,
            conversations,
            conversations_state,
            conversations_offset: 0,
            conversations_exhausted,
            messages: Vec::new(),
            message_selected: 0,
            reader_scroll: 0,
            reader_wrap_width: 100,
            conversation_id: None,
            conversation_title: String::new(),
            reader_mode: ReaderMode::Thread,
            focus: Focus::Conversations,
            overlay: None,
            technical_visible: false,
            total_conversations,
            conversation_message_count: 0,
            messages_capped: false,
            branch_alternatives: None,
            branch_hint_node: None,
            status: String::new(),
            assets: Vec::new(),
            should_quit: false,
            clipboard: Clipboard::new().ok(),
            messages_generation: 0,
            line_count_cache: Vec::new(),
            line_count_cache_width: 0,
            line_count_cache_technical: false,
            line_count_cache_generation: 0,
            reader_render_cache: None,
            thread_cache: HashMap::new(),
            thread_cache_order: Vec::new(),
            pending_conversation_id: None,
            conversation_nav_at: Instant::now(),
            loading_conversation: false,
            search_input: TextArea::default(),
            input_mode: InputMode::Search,
            conversations_rect: Rect::default(),
            reader_rect: Rect::default(),
            inspector_rect: Rect::default(),
            input_rect: Rect::default(),
        };
        app.refresh_status();
        if let Some(conversation) = app.conversations.first() {
            app.pending_conversation_id = Some(conversation.id.clone());
            app.conversation_nav_at = Instant::now();
        }
        Ok(app)
    }

    pub fn invalidate_reader_cache(&mut self) {
        self.reader_render_cache = None;
        self.line_count_cache.clear();
        self.line_count_cache_generation = 0;
    }

    pub fn bump_messages_generation(&mut self) {
        self.messages_generation = self.messages_generation.wrapping_add(1);
        self.invalidate_reader_cache();
        self.branch_hint_node = None;
    }

    pub fn thread_cache_key(conversation_id: &str, mode: ReaderMode) -> String {
        match mode {
            ReaderMode::Thread => format!("{conversation_id}:thread"),
            ReaderMode::AllMessages => format!("{conversation_id}:all"),
        }
    }

    pub fn message_thread_cache_key(conversation_id: &str, message_id: &str) -> String {
        format!("{conversation_id}:msg:{message_id}")
    }

    pub fn cached_message_thread(
        &mut self,
        conversation_id: &str,
        message_id: &str,
    ) -> Option<Vec<MessageView>> {
        let key = Self::message_thread_cache_key(conversation_id, message_id);
        self.cached_thread_entry(&key)
    }

    pub fn store_message_thread_cache(
        &mut self,
        conversation_id: &str,
        message_id: &str,
        messages: &[MessageView],
    ) {
        self.store_thread_cache_entry(
            Self::message_thread_cache_key(conversation_id, message_id),
            messages,
        );
    }

    pub fn store_thread_cache(
        &mut self,
        conversation_id: &str,
        mode: ReaderMode,
        messages: &[MessageView],
    ) {
        self.store_thread_cache_entry(Self::thread_cache_key(conversation_id, mode), messages);
    }

    pub fn store_thread_cache_entry(&mut self, key: String, messages: &[MessageView]) {
        if self.thread_cache.contains_key(&key) {
            self.thread_cache_order.retain(|id| id != &key);
        }
        self.thread_cache_order.push(key.clone());
        while self.thread_cache_order.len() > THREAD_CACHE_LIMIT {
            if let Some(evicted) = self.thread_cache_order.first().cloned() {
                self.thread_cache_order.remove(0);
                self.thread_cache.remove(&evicted);
            }
        }
        self.thread_cache.insert(
            key,
            ThreadCacheEntry {
                messages: messages.to_vec(),
            },
        );
    }

    pub fn cached_thread(
        &mut self,
        conversation_id: &str,
        mode: ReaderMode,
    ) -> Option<Vec<MessageView>> {
        let key = Self::thread_cache_key(conversation_id, mode);
        self.cached_thread_entry(&key)
    }

    pub fn cached_thread_entry(&mut self, key: &str) -> Option<Vec<MessageView>> {
        let messages = self.thread_cache.get(key)?.messages.clone();
        self.thread_cache_order.retain(|candidate| candidate != key);
        self.thread_cache_order.push(key.to_owned());
        Some(messages)
    }

    pub fn index_of_conversation(&self, conversation_id: &str) -> Option<usize> {
        self.conversations
            .iter()
            .position(|conversation| conversation.id == conversation_id)
    }

    pub fn selected_conversation_id(&self) -> Option<&str> {
        self.conversations_state
            .selected()
            .and_then(|index| self.conversations.get(index))
            .map(|conversation| conversation.id.as_str())
    }

    pub fn refresh_status(&mut self) {
        let technical = if self.technical_visible {
            " · technical visible"
        } else {
            ""
        };
        if self.conversation_id.is_some() {
            self.status = format!(
                "{}{} · {} · / search · ? help",
                self.reader_mode.label(),
                technical,
                self.reader_context_label()
            );
            return;
        }
        let loaded = self.conversations.len();
        self.status = if self.conversations_exhausted {
            format!(
                "{} conversations · / search · ? help",
                self.total_conversations
            )
        } else {
            format!(
                "{loaded}/{} loaded · ↓ scroll · / search · ? help",
                self.total_conversations
            )
        };
    }

    pub fn conversation_list_position(&self) -> String {
        let selected = self
            .conversations_state
            .selected()
            .map(|index| index + 1)
            .unwrap_or(0);
        let loaded = self.conversations.len();
        if self.conversations_exhausted {
            format!("{selected}/{}", self.total_conversations)
        } else {
            format!("{selected}/{loaded}+")
        }
    }

    pub fn reader_context_label(&self) -> String {
        if self.loading_conversation {
            return format!("Loading · {}", truncate(&self.conversation_title, 36));
        }
        let title = truncate(&self.conversation_title, 28);
        if self.messages.is_empty() {
            return title;
        }
        let navigable = self.navigable_message_indices();
        let position = navigable
            .iter()
            .position(|index| *index == self.message_selected)
            .map(|index| index + 1)
            .unwrap_or(1);
        let mut label = format!("{title} · msg {position}/{}", navigable.len());
        if self.messages_capped {
            label.push_str(&format!(
                " · showing {}/{} msgs",
                self.messages.len(),
                self.conversation_message_count
            ));
        } else if self.reader_mode == ReaderMode::AllMessages {
            label.push_str(&format!(
                " · {}/{} msgs",
                self.messages.len(),
                self.conversation_message_count
            ));
        }
        label
    }

    pub fn conversations_panel_title(&self) -> String {
        if self.conversations_exhausted {
            format!("Conversations · {}", self.conversation_list_position())
        } else {
            format!(
                "Conversations · {} · ↓ more",
                self.conversation_list_position()
            )
        }
    }

    pub fn reader_panel_title(&self) -> String {
        if self.conversation_id.is_none() {
            return "Reader · select a conversation".into();
        }
        if self.loading_conversation {
            return format!("Loading · {}", truncate(&self.conversation_title, 36));
        }
        let mut title = format!(
            "{} · {}",
            self.reader_mode.label(),
            truncate(&self.conversation_title, 32)
        );
        if self.messages_capped {
            title.push_str(&format!(
                " · capped at {}/{}",
                self.messages.len(),
                self.conversation_message_count
            ));
        }
        title
    }

    pub fn update_messages_cap(&mut self) {
        self.messages_capped = self.reader_mode == ReaderMode::AllMessages
            && self.messages.len() >= ALL_MESSAGES_CAP
            && self.conversation_message_count > ALL_MESSAGES_CAP as i64;
    }

    pub fn conversation_message_count_for(&self, conversation_id: &str) -> i64 {
        self.conversations
            .iter()
            .find(|conversation| conversation.id == conversation_id)
            .map(|conversation| conversation.message_count)
            .unwrap_or(0)
    }

    pub fn refresh_branch_hint(&mut self) -> Result<()> {
        let Some(message) = self.selected_message().cloned() else {
            self.branch_alternatives = None;
            self.branch_hint_node = None;
            return Ok(());
        };
        if self.branch_hint_node.as_deref() == Some(message.node_id.as_str()) {
            return Ok(());
        }
        self.branch_hint_node = Some(message.node_id.clone());
        let choices = self.repository.branches_on_path(&message.node_id)?;
        let new_alternatives = (choices.len() >= 2).then_some(choices.len());
        if self.branch_alternatives != new_alternatives {
            self.reader_render_cache = None;
        }
        self.branch_alternatives = new_alternatives;
        Ok(())
    }

    pub fn navigable_message_indices(&self) -> Vec<usize> {
        if self.technical_visible {
            return (0..self.messages.len()).collect();
        }
        let mut indices = Vec::new();
        let mut index = 0;
        while index < self.messages.len() {
            if self.messages[index].is_technical() {
                indices.push(index);
                while index < self.messages.len() && self.messages[index].is_technical() {
                    index += 1;
                }
            } else {
                indices.push(index);
                index += 1;
            }
        }
        indices
    }

    pub fn ensure_line_counts(&mut self, wrap_width: usize) {
        let width = u16::try_from(wrap_width).unwrap_or(u16::MAX);
        if self.line_count_cache_generation == self.messages_generation
            && self.line_count_cache_width == width
            && self.line_count_cache_technical == self.technical_visible
            && !self.line_count_cache.is_empty()
        {
            return;
        }
        self.line_count_cache = self
            .navigable_message_indices()
            .into_iter()
            .map(|index| {
                let message = &self.messages[index];
                if message.is_technical() && !self.technical_visible {
                    1
                } else {
                    1 + wrap_content(&message.content, wrap_width).len() + 1
                }
            })
            .collect();
        self.line_count_cache_width = width;
        self.line_count_cache_technical = self.technical_visible;
        self.line_count_cache_generation = self.messages_generation;
    }

    pub fn reader_lines(&mut self, wrap_width: usize) -> Vec<Line<'static>> {
        let width = u16::try_from(wrap_width).unwrap_or(u16::MAX);
        if let Some(cache) = &self.reader_render_cache {
            if cache.wrap_width == width
                && cache.technical_visible == self.technical_visible
                && cache.messages_generation == self.messages_generation
                && cache.message_selected == self.message_selected
                && cache.branch_alternatives == self.branch_alternatives
            {
                return cache.lines.clone();
            }
        }
        let lines = build_reader_lines(self, wrap_width);
        self.reader_render_cache = Some(ReaderRenderCache {
            wrap_width: width,
            technical_visible: self.technical_visible,
            messages_generation: self.messages_generation,
            message_selected: self.message_selected,
            branch_alternatives: self.branch_alternatives,
            lines: lines.clone(),
        });
        lines
    }

    pub fn visible_message_indices(&self) -> Vec<usize> {
        self.navigable_message_indices()
    }

    pub fn selected_message(&self) -> Option<&MessageView> {
        self.messages.get(self.message_selected)
    }

    pub fn refresh_inspector(&mut self) -> Result<()> {
        if self.focus == Focus::Inspector {
            self.assets = self
                .selected_message()
                .map(|message| self.repository.message_assets(&message.id))
                .transpose()?
                .unwrap_or_default();
        }
        self.refresh_branch_hint()?;
        Ok(())
    }

    pub fn update_reader_scroll(&mut self) {
        let wrap = usize::from(self.reader_wrap_width.saturating_sub(4).max(20));
        self.ensure_line_counts(wrap);
        let visible = self.navigable_message_indices();
        if visible.is_empty() {
            self.reader_scroll = 0;
            return;
        }
        if !visible.contains(&self.message_selected) {
            self.message_selected = visible[0];
        }
        let lines_before = visible
            .iter()
            .position(|index| *index == self.message_selected)
            .map(|position| {
                (0..position)
                    .map(|nav_index| self.line_count_cache.get(nav_index).copied().unwrap_or(1))
                    .sum::<usize>()
            })
            .unwrap_or(0);
        self.reader_scroll = u16::try_from(lines_before.saturating_sub(3)).unwrap_or(u16::MAX);
    }
}
