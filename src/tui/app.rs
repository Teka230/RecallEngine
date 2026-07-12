use std::{
    collections::HashMap,
    io::{self, stdout},
    path::PathBuf,
    time::{Duration, Instant},
};

use arboard::Clipboard;
use ratatui::crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};

use crate::{
    domain::reference::MessageReference,
    read_model::{
        AssetView, BranchChoice, ConversationListItem, IcJumpTarget, MessageView, ReadRepository,
        SearchHit,
    },
    Result,
};

const PAGE_SIZE: usize = 150;
const ALL_MESSAGES_CAP: usize = 500;
const THREAD_CACHE_LIMIT: usize = 12;
const CONVERSATION_LOAD_DEBOUNCE: Duration = Duration::from_millis(180);

#[derive(Clone)]
struct ThreadCacheEntry {
    messages: Vec<MessageView>,
}

#[derive(Clone)]
struct ReaderRenderCache {
    wrap_width: u16,
    technical_visible: bool,
    messages_generation: u64,
    message_selected: usize,
    branch_alternatives: Option<usize>,
    lines: Vec<Line<'static>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReaderMode {
    Thread,
    AllMessages,
}

impl ReaderMode {
    fn label(self) -> &'static str {
        match self {
            Self::Thread => "Thread",
            Self::AllMessages => "All messages · IC order",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Conversations,
    Reader,
    Inspector,
}

#[derive(Debug)]
enum Overlay {
    Search {
        value: String,
    },
    JumpIc {
        value: String,
    },
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

struct App {
    repository: ReadRepository,
    conversations: Vec<ConversationListItem>,
    conversations_state: ListState,
    conversations_offset: usize,
    conversations_exhausted: bool,
    messages: Vec<MessageView>,
    message_selected: usize,
    reader_scroll: u16,
    reader_wrap_width: u16,
    conversation_id: Option<String>,
    conversation_title: String,
    reader_mode: ReaderMode,
    focus: Focus,
    overlay: Option<Overlay>,
    technical_visible: bool,
    total_conversations: i64,
    conversation_message_count: i64,
    messages_capped: bool,
    branch_alternatives: Option<usize>,
    branch_hint_node: Option<String>,
    status: String,
    assets: Vec<AssetView>,
    should_quit: bool,
    clipboard: Option<Clipboard>,
    messages_generation: u64,
    line_count_cache: Vec<usize>,
    line_count_cache_width: u16,
    line_count_cache_technical: bool,
    line_count_cache_generation: u64,
    reader_render_cache: Option<ReaderRenderCache>,
    thread_cache: HashMap<String, ThreadCacheEntry>,
    thread_cache_order: Vec<String>,
    pending_conversation_id: Option<String>,
    conversation_nav_at: Instant,
    loading_conversation: bool,
}

pub fn run(db_path: PathBuf, ic: Option<i64>, conversation: Option<String>) -> Result<()> {
    let repository = ReadRepository::open_read_only(&db_path)?;
    let mut app = App::new(repository)?;
    if let Some(ic) = ic {
        app.open_ic(ic)?;
    } else if let Some(conversation) = conversation {
        app.load_conversation(&conversation, true)?;
    }
    run_terminal(&mut app)
}

impl Focus {
    fn label(self) -> &'static str {
        match self {
            Self::Conversations => "Conversations",
            Self::Reader => "Reader",
            Self::Inspector => "Inspector",
        }
    }
}

impl App {
    fn new(repository: ReadRepository) -> Result<Self> {
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
        };
        app.refresh_status();
        if let Some(conversation) = app.conversations.first() {
            app.pending_conversation_id = Some(conversation.id.clone());
            app.conversation_nav_at = Instant::now();
        }
        Ok(app)
    }

    fn invalidate_reader_cache(&mut self) {
        self.reader_render_cache = None;
        self.line_count_cache.clear();
        self.line_count_cache_generation = 0;
    }

    fn bump_messages_generation(&mut self) {
        self.messages_generation = self.messages_generation.wrapping_add(1);
        self.invalidate_reader_cache();
        self.branch_hint_node = None;
    }

    fn thread_cache_key(conversation_id: &str, mode: ReaderMode) -> String {
        match mode {
            ReaderMode::Thread => format!("{conversation_id}:thread"),
            ReaderMode::AllMessages => format!("{conversation_id}:all"),
        }
    }

    fn message_thread_cache_key(conversation_id: &str, message_id: &str) -> String {
        format!("{conversation_id}:msg:{message_id}")
    }

    fn cached_message_thread(
        &mut self,
        conversation_id: &str,
        message_id: &str,
    ) -> Option<Vec<MessageView>> {
        let key = Self::message_thread_cache_key(conversation_id, message_id);
        self.cached_thread_entry(&key)
    }

    fn store_message_thread_cache(
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

    fn store_thread_cache(
        &mut self,
        conversation_id: &str,
        mode: ReaderMode,
        messages: &[MessageView],
    ) {
        self.store_thread_cache_entry(Self::thread_cache_key(conversation_id, mode), messages);
    }

    fn store_thread_cache_entry(&mut self, key: String, messages: &[MessageView]) {
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

    fn cached_thread(
        &mut self,
        conversation_id: &str,
        mode: ReaderMode,
    ) -> Option<Vec<MessageView>> {
        let key = Self::thread_cache_key(conversation_id, mode);
        self.cached_thread_entry(&key)
    }

    fn cached_thread_entry(&mut self, key: &str) -> Option<Vec<MessageView>> {
        let messages = self.thread_cache.get(key)?.messages.clone();
        self.thread_cache_order.retain(|candidate| candidate != key);
        self.thread_cache_order.push(key.to_owned());
        Some(messages)
    }

    fn schedule_conversation_load(&mut self, conversation_id: String) {
        if let Some(conversation) = self
            .conversations
            .iter()
            .find(|conversation| conversation.id == conversation_id)
        {
            self.conversation_title = conversation.title.clone();
            self.conversation_message_count = conversation.message_count;
        }
        self.pending_conversation_id = Some(conversation_id);
        self.conversation_nav_at = Instant::now();
        self.loading_conversation = true;
        self.refresh_status();
    }

    fn flush_pending_conversation_load(&mut self) -> Result<()> {
        let Some(conversation_id) = self.pending_conversation_id.clone() else {
            return Ok(());
        };
        if self.conversation_nav_at.elapsed() < CONVERSATION_LOAD_DEBOUNCE {
            return Ok(());
        }
        self.pending_conversation_id = None;
        self.loading_conversation = false;
        self.load_conversation(&conversation_id, false)
    }

    fn index_of_conversation(&self, conversation_id: &str) -> Option<usize> {
        self.conversations
            .iter()
            .position(|conversation| conversation.id == conversation_id)
    }

    fn selected_conversation_id(&self) -> Option<&str> {
        self.conversations_state
            .selected()
            .and_then(|index| self.conversations.get(index))
            .map(|conversation| conversation.id.as_str())
    }

    fn sync_sidebar_to_active(&mut self) -> Result<()> {
        let Some(active_id) = self.conversation_id.clone() else {
            return Ok(());
        };
        if let Some(index) = self.index_of_conversation(&active_id) {
            self.conversations_state.select(Some(index));
            return Ok(());
        }
        let Some(index) = self.repository.conversation_list_index(&active_id)? else {
            return Ok(());
        };
        self.jump_sidebar_to_index(index, &active_id)
    }

    fn jump_sidebar_to_index(&mut self, index: usize, conversation_id: &str) -> Result<()> {
        let page_start = (index / PAGE_SIZE) * PAGE_SIZE;
        if self.conversations_offset == 0 && index < self.conversations.len() {
            if let Some(local_index) = self.index_of_conversation(conversation_id) {
                self.conversations_state.select(Some(local_index));
                return Ok(());
            }
        }
        let page = self
            .repository
            .list_conversations_page("", PAGE_SIZE, page_start)?;
        self.conversations = page;
        self.conversations_offset = page_start;
        self.conversations_exhausted = self.conversations_offset + self.conversations.len()
            >= self.total_conversations as usize;
        if let Some(local_index) = self.index_of_conversation(conversation_id) {
            self.conversations_state.select(Some(local_index));
        }
        Ok(())
    }

    fn refresh_status(&mut self) {
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

    fn conversation_list_position(&self) -> String {
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

    fn reader_context_label(&self) -> String {
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

    fn conversations_panel_title(&self) -> String {
        if self.conversations_exhausted {
            format!("Conversations · {}", self.conversation_list_position())
        } else {
            format!(
                "Conversations · {} · ↓ more",
                self.conversation_list_position()
            )
        }
    }

    fn reader_panel_title(&self) -> String {
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

    fn update_messages_cap(&mut self) {
        self.messages_capped = self.reader_mode == ReaderMode::AllMessages
            && self.messages.len() >= ALL_MESSAGES_CAP
            && self.conversation_message_count > ALL_MESSAGES_CAP as i64;
    }

    fn conversation_message_count_for(&self, conversation_id: &str) -> i64 {
        self.conversations
            .iter()
            .find(|conversation| conversation.id == conversation_id)
            .map(|conversation| conversation.message_count)
            .unwrap_or(0)
    }

    fn refresh_branch_hint(&mut self) -> Result<()> {
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

    fn navigable_message_indices(&self) -> Vec<usize> {
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

    fn ensure_line_counts(&mut self, wrap_width: usize) {
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

    fn reader_lines(&mut self, wrap_width: usize) -> Vec<Line<'static>> {
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

    fn load_conversation(&mut self, conversation_id: &str, focus_reader: bool) -> Result<()> {
        let Some(meta) = self.repository.conversation_meta(conversation_id)? else {
            self.status = "Conversation not found".into();
            self.loading_conversation = false;
            return Ok(());
        };
        self.conversation_id = Some(meta.id.clone());
        self.conversation_title = meta.title;
        self.conversation_message_count = self.conversation_message_count_for(conversation_id);
        self.reader_mode = ReaderMode::Thread;
        self.messages =
            if let Some(cached) = self.cached_thread(conversation_id, ReaderMode::Thread) {
                cached
            } else {
                let loaded = self.repository.current_thread(conversation_id)?;
                self.store_thread_cache(conversation_id, ReaderMode::Thread, &loaded);
                loaded
            };
        self.bump_messages_generation();
        self.update_messages_cap();
        if focus_reader {
            self.focus = Focus::Reader;
        }
        self.sync_sidebar_to_active()?;
        self.show_first_message()?;
        self.loading_conversation = false;
        self.refresh_status();
        Ok(())
    }

    fn open_ic(&mut self, ic: i64) -> Result<()> {
        let Some(target) = self.repository.resolve_ic_jump(ic)? else {
            self.status = format!("IC {ic} not found");
            return Ok(());
        };
        self.open_jump_target(target, format!("IC {ic}"))
    }

    fn open_message_id(&mut self, message_id: &str) -> Result<()> {
        let Some(target) = self.repository.resolve_message_id_jump(message_id)? else {
            self.status = format!("Message {message_id} not found");
            return Ok(());
        };
        self.open_jump_target(target, format!("msg:{message_id}"))
    }

    fn open_reference(&mut self, reference: &MessageReference) -> Result<()> {
        let target = match self.repository.resolve_reference_jump(reference) {
            Ok(Some(target)) => target,
            Ok(None) => {
                self.status = format!("Message {} not found", reference.message_id);
                return Ok(());
            }
            Err(error) => {
                self.status = error.to_string();
                return Ok(());
            }
        };
        self.open_jump_target(target, reference.human())
    }

    fn open_jump_target(&mut self, target: IcJumpTarget, label: String) -> Result<()> {
        self.overlay = None;
        self.conversation_id = Some(target.conversation_id.clone());
        self.conversation_title = self
            .repository
            .conversation_meta(&target.conversation_id)?
            .map(|meta| meta.title)
            .unwrap_or_else(|| "Untitled".into());
        self.conversation_message_count =
            self.conversation_message_count_for(&target.conversation_id);
        self.reader_mode = ReaderMode::Thread;
        self.messages = if let Some(cached) =
            self.cached_message_thread(&target.conversation_id, &target.message_id)
        {
            cached
        } else {
            let loaded = self.repository.thread_for_message(&target.message_id)?;
            self.store_message_thread_cache(&target.conversation_id, &target.message_id, &loaded);
            loaded
        };
        self.bump_messages_generation();
        self.update_messages_cap();
        self.message_selected = self
            .messages
            .iter()
            .position(|candidate| candidate.id == target.message_id)
            .unwrap_or(0);
        self.focus = Focus::Reader;
        self.refresh_branch_hint()?;
        self.update_reader_scroll();
        self.status = format!("{label} · {} · / search · ? help", self.reader_mode.label());
        Ok(())
    }

    fn visible_message_indices(&self) -> Vec<usize> {
        self.navigable_message_indices()
    }

    fn selected_message(&self) -> Option<&MessageView> {
        self.messages.get(self.message_selected)
    }

    fn refresh_inspector(&mut self) -> Result<()> {
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

    fn update_reader_scroll(&mut self) {
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

    fn show_first_message(&mut self) -> Result<()> {
        self.message_selected = 0;
        self.reader_scroll = 0;
        self.refresh_inspector()?;
        Ok(())
    }

    fn move_message(&mut self, delta: isize) -> Result<()> {
        let visible = self.visible_message_indices();
        if visible.is_empty() {
            return Ok(());
        }
        let current = visible
            .iter()
            .position(|index| *index == self.message_selected)
            .unwrap_or(0) as isize;
        let target = (current + delta).clamp(0, visible.len().saturating_sub(1) as isize) as usize;
        self.message_selected = visible[target];
        self.reader_render_cache = None;
        self.refresh_inspector()?;
        self.update_reader_scroll();
        Ok(())
    }

    fn load_more_conversations(&mut self) -> Result<()> {
        if self.conversations_exhausted {
            return Ok(());
        }
        let page = self.repository.list_conversations_page(
            "",
            PAGE_SIZE,
            self.conversations_offset + self.conversations.len(),
        )?;
        if page.len() < PAGE_SIZE {
            self.conversations_exhausted = true;
        }
        self.conversations.extend(page);
        Ok(())
    }

    fn move_conversation(&mut self, delta: isize) -> Result<()> {
        let len = self.conversations.len();
        if len == 0 {
            return Ok(());
        }
        if delta > 0 && self.conversations_state.selected() == Some(len - 1) {
            self.load_more_conversations()?;
        }
        let len = self.conversations.len();
        let current = self.conversations_state.selected().unwrap_or(0) as isize;
        let target = (current + delta).clamp(0, len.saturating_sub(1) as isize) as usize;
        self.conversations_state.select(Some(target));
        if let Some(conversation) = self.conversations.get(target) {
            let id = conversation.id.clone();
            if self.conversation_id.as_deref() == Some(id.as_str()) {
                self.refresh_status();
            } else {
                self.schedule_conversation_load(id);
            }
        }
        Ok(())
    }

    fn open_search_hit(&mut self, message_id: &str) -> Result<()> {
        self.overlay = None;
        let active_conversation_id = self.conversation_id.clone();
        let cached = active_conversation_id
            .as_deref()
            .and_then(|conversation_id| self.cached_message_thread(conversation_id, message_id));
        self.messages = if let Some(cached) = cached {
            cached
        } else {
            let loaded = self.repository.thread_for_message(message_id)?;
            if let Some(conversation_id) = loaded
                .iter()
                .find(|message| message.id == message_id)
                .map(|message| message.conversation_id.clone())
            {
                self.store_message_thread_cache(&conversation_id, message_id, &loaded);
            }
            loaded
        };
        self.bump_messages_generation();
        let Some(message) = self
            .messages
            .iter()
            .find(|message| message.id == message_id)
            .cloned()
        else {
            self.status = "Message is no longer active".into();
            return Ok(());
        };
        self.conversation_id = Some(message.conversation_id.clone());
        self.conversation_title = self
            .repository
            .conversation_meta(&message.conversation_id)?
            .map(|meta| meta.title)
            .unwrap_or_else(|| "Untitled".into());
        self.conversation_message_count =
            self.conversation_message_count_for(&message.conversation_id);
        self.message_selected = self
            .messages
            .iter()
            .position(|candidate| candidate.id == message_id)
            .unwrap_or(0);
        self.reader_mode = ReaderMode::Thread;
        self.update_messages_cap();
        self.focus = Focus::Reader;
        self.refresh_branch_hint()?;
        self.update_reader_scroll();
        self.refresh_status();
        Ok(())
    }

    fn toggle_mode(&mut self) -> Result<()> {
        let Some(conversation_id) = self.conversation_id.clone() else {
            return Ok(());
        };
        let selected_id = self.selected_message().map(|message| message.id.clone());
        self.reader_mode = match self.reader_mode {
            ReaderMode::Thread => ReaderMode::AllMessages,
            ReaderMode::AllMessages => ReaderMode::Thread,
        };
        self.messages = match self.reader_mode {
            ReaderMode::Thread => {
                if let Some(cached) = self.cached_thread(&conversation_id, ReaderMode::Thread) {
                    cached
                } else {
                    let loaded = self.repository.current_thread(&conversation_id)?;
                    self.store_thread_cache(&conversation_id, ReaderMode::Thread, &loaded);
                    loaded
                }
            }
            ReaderMode::AllMessages => {
                if let Some(cached) = self.cached_thread(&conversation_id, ReaderMode::AllMessages)
                {
                    cached
                } else {
                    let loaded = self
                        .repository
                        .all_messages(&conversation_id, ALL_MESSAGES_CAP)?;
                    self.store_thread_cache(&conversation_id, ReaderMode::AllMessages, &loaded);
                    loaded
                }
            }
        };
        self.bump_messages_generation();
        self.update_messages_cap();
        if let Some(id) = selected_id.as_deref() {
            if let Some(index) = self.messages.iter().position(|message| message.id == id) {
                self.message_selected = index;
                self.update_reader_scroll();
                self.refresh_inspector()?;
            } else {
                self.show_first_message()?;
            }
        } else {
            self.show_first_message()?;
        }
        self.refresh_status();
        Ok(())
    }

    fn open_branches(&mut self) -> Result<()> {
        let Some(message) = self.selected_message() else {
            return Ok(());
        };
        let choices = self.repository.branches_on_path(&message.node_id)?;
        if choices.len() < 2 {
            self.status = "No branch alternatives on this path".into();
        } else {
            self.overlay = Some(Overlay::Branches {
                choices,
                selected: 0,
            });
        }
        Ok(())
    }

    fn copy_citation(&mut self) {
        let Some(message) = self.selected_message() else {
            self.status = "No message selected".into();
            return;
        };
        let Some(ic) = message.ic else {
            self.status = "No stable IC reference on selected message".into();
            return;
        };
        let citation = MessageReference::new(ic, message.id.clone())
            .expect("selected public message has a valid reference")
            .human();
        let Some(clipboard) = self.clipboard.as_mut() else {
            self.status = format!("Clipboard unavailable — reference: {citation}");
            return;
        };
        match clipboard.set_text(citation.clone()) {
            Ok(()) => self.status = format!("Copied {citation}"),
            Err(_) => self.status = format!("Clipboard unavailable — reference: {citation}"),
        }
    }

    fn open_context(&mut self) -> Result<()> {
        let Some(ic) = self.selected_message().and_then(|message| message.ic) else {
            self.status = "No stable IC reference on selected message".into();
            return Ok(());
        };
        let Some(context) = self.repository.ic_context(ic)? else {
            self.status = format!("IC {ic} not found");
            return Ok(());
        };
        let lines = context
            .messages
            .iter()
            .map(|message| {
                format!(
                    "{} {} · {}",
                    message.reference,
                    message.role,
                    first_line(&message.content, 70)
                )
            })
            .collect();
        self.overlay = Some(Overlay::Context { lines });
        Ok(())
    }

    fn start_search(&mut self) {
        self.overlay = Some(Overlay::Search {
            value: String::new(),
        });
    }

    fn start_ic_jump(&mut self) {
        self.overlay = Some(Overlay::JumpIc {
            value: String::new(),
        });
    }

    fn submit_search(&mut self, query: String) -> Result<()> {
        let trimmed = query.trim();
        if parse_ic_query(trimmed).is_some()
            || trimmed.starts_with("msg:")
            || trimmed.starts_with("ref:ic/")
            || (trimmed.starts_with("[IC:") && trimmed.contains(" | msg:"))
        {
            return self.open_reference_input(trimmed, false);
        }
        let hits = self.repository.search(&query, PAGE_SIZE)?;
        self.overlay = Some(Overlay::SearchResults {
            query,
            hits,
            selected: 0,
        });
        Ok(())
    }

    fn open_reference_input(&mut self, value: &str, allow_raw_message_id: bool) -> Result<()> {
        let value = value.trim();
        if let Ok(reference) = value.parse::<MessageReference>() {
            return self.open_reference(&reference);
        }
        if let Some(ic) = parse_ic_query(value) {
            return self.open_ic(ic);
        }
        if let Some(message_id) = value.strip_prefix("msg:") {
            return self.open_message_id(message_id);
        }
        if allow_raw_message_id && !value.is_empty() {
            return self.open_message_id(value);
        }
        self.status = "Enter an IC, msg:<id>, or composite reference".into();
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.handle_overlay_key(key)? {
            return Ok(());
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help),
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Conversations => Focus::Reader,
                    Focus::Reader => Focus::Inspector,
                    Focus::Inspector => Focus::Conversations,
                };
                if self.focus == Focus::Inspector {
                    self.refresh_inspector()?;
                }
                self.refresh_status();
            }
            KeyCode::Esc => {
                self.sync_sidebar_to_active()?;
                self.refresh_status();
            }
            KeyCode::Char('/') => self.start_search(),
            KeyCode::Char('i') => self.start_ic_jump(),
            KeyCode::Char('v') => self.toggle_mode()?,
            KeyCode::Char('t') => {
                self.technical_visible = !self.technical_visible;
                self.invalidate_reader_cache();
                if !self.technical_visible
                    && self
                        .selected_message()
                        .is_some_and(MessageView::is_technical)
                {
                    self.move_message(-1)?;
                } else {
                    self.update_reader_scroll();
                }
                self.refresh_status();
            }
            KeyCode::Char('b') => self.open_branches()?,
            KeyCode::Char('y') => self.copy_citation(),
            KeyCode::Char('d') => {
                self.focus = Focus::Inspector;
                self.refresh_inspector()?;
            }
            KeyCode::Char('c') => self.open_context()?,
            KeyCode::Char('j') | KeyCode::Down => match self.focus {
                Focus::Conversations => self.move_conversation(1)?,
                Focus::Reader | Focus::Inspector => self.move_message(1)?,
            },
            KeyCode::Char('k') | KeyCode::Up => match self.focus {
                Focus::Conversations => self.move_conversation(-1)?,
                Focus::Reader | Focus::Inspector => self.move_message(-1)?,
            },
            KeyCode::Enter if self.focus == Focus::Conversations => {
                if let Some(id) = self
                    .pending_conversation_id
                    .take()
                    .or_else(|| self.selected_conversation_id().map(str::to_owned))
                {
                    self.loading_conversation = false;
                    if self.conversation_id.as_deref() != Some(id.as_str()) {
                        self.load_conversation(&id, true)?;
                    } else {
                        self.focus = Focus::Reader;
                        self.refresh_status();
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) -> Result<bool> {
        let Some(overlay) = self.overlay.take() else {
            return Ok(false);
        };
        match overlay {
            Overlay::Search { mut value } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => self.submit_search(value)?,
                KeyCode::Backspace => {
                    value.pop();
                    self.overlay = Some(Overlay::Search { value });
                }
                KeyCode::Char(character)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    value.push(character);
                    self.overlay = Some(Overlay::Search { value });
                }
                _ => self.overlay = Some(Overlay::Search { value }),
            },
            Overlay::JumpIc { mut value } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => self.open_reference_input(&value, true)?,
                KeyCode::Backspace => {
                    value.pop();
                    self.overlay = Some(Overlay::JumpIc { value });
                }
                KeyCode::Char(character)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    value.push(character);
                    self.overlay = Some(Overlay::JumpIc { value });
                }
                _ => self.overlay = Some(Overlay::JumpIc { value }),
            },
            Overlay::Branches {
                choices,
                mut selected,
            } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Char('j') | KeyCode::Down => {
                    selected = (selected + 1).min(choices.len().saturating_sub(1));
                    self.overlay = Some(Overlay::Branches { choices, selected });
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                    self.overlay = Some(Overlay::Branches { choices, selected });
                }
                KeyCode::Enter => {
                    if let Some(choice) = choices.get(selected) {
                        let node_id = choice.node_id.clone();
                        self.messages = self.repository.thread_for_node(&node_id)?;
                        self.bump_messages_generation();
                        self.reader_mode = ReaderMode::Thread;
                        self.focus = Focus::Reader;
                        self.sync_sidebar_to_active()?;
                        self.show_first_message()?;
                        self.refresh_status();
                    }
                }
                _ => self.overlay = Some(Overlay::Branches { choices, selected }),
            },
            Overlay::Context { lines } => match key.code {
                KeyCode::Esc | KeyCode::Char('c') | KeyCode::Enter => {}
                _ => self.overlay = Some(Overlay::Context { lines }),
            },
            Overlay::SearchResults {
                query,
                hits,
                mut selected,
            } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Char('j') | KeyCode::Down => {
                    selected = (selected + 1).min(hits.len().saturating_sub(1));
                    self.overlay = Some(Overlay::SearchResults {
                        query,
                        hits,
                        selected,
                    });
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                    self.overlay = Some(Overlay::SearchResults {
                        query,
                        hits,
                        selected,
                    });
                }
                KeyCode::Enter => {
                    if let Some(hit) = hits.get(selected) {
                        let message_id = hit.message_id.clone();
                        self.open_search_hit(&message_id)?;
                    }
                }
                _ => {
                    self.overlay = Some(Overlay::SearchResults {
                        query,
                        hits,
                        selected,
                    });
                }
            },
            Overlay::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Enter => {}
                _ => self.overlay = Some(Overlay::Help),
            },
        }
        Ok(true)
    }
}

fn run_terminal(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut out = stdout();
    if let Err(error) = execute!(out, EnterAlternateScreen, ratatui::crossterm::cursor::Hide) {
        let _ = disable_raw_mode();
        return Err(error.into());
    }
    let backend = CrosstermBackend::new(out);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = disable_raw_mode();
            let mut out = stdout();
            let _ = execute!(out, LeaveAlternateScreen, ratatui::crossterm::cursor::Show);
            return Err(error.into());
        }
    };
    let outcome = run_loop(&mut terminal, app);
    let restore = restore_terminal(&mut terminal);
    outcome.and(restore)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        ratatui::crossterm::cursor::Show
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    while !app.should_quit {
        app.flush_pending_conversation_load()?;
        terminal.draw(|frame| render(frame, app))?;
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == event::KeyEventKind::Press {
                    app.handle_key(key)?;
                }
            }
        }
    }
    Ok(())
}

fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 60 || area.height < 15 {
        frame.render_widget(
            Paragraph::new("Terminal too small\nResize to at least 60 × 15, then press q to quit.")
                .block(
                    Block::default()
                        .title("RecallEngine browse")
                        .borders(Borders::ALL),
                )
                .alignment(ratatui::layout::Alignment::Center),
            area,
        );
        return;
    }
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    if area.width < 80 {
        match app.focus {
            Focus::Conversations => render_conversations(frame, app, vertical[0]),
            Focus::Reader => render_reader(frame, app, vertical[0]),
            Focus::Inspector => render_inspector(frame, app, vertical[0]),
        }
        frame.render_widget(
            Paragraph::new(status_line(app)).style(Style::default().fg(Color::DarkGray)),
            vertical[1],
        );
        if let Some(overlay) = &app.overlay {
            render_overlay(frame, overlay, centered_rect(92, 70, area));
        }
        return;
    }
    let main = if area.width >= 120 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(28),
                Constraint::Percentage(52),
                Constraint::Percentage(20),
            ])
            .split(vertical[0])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
            .split(vertical[0])
    };
    render_conversations(frame, app, main[0]);
    if main.len() == 2 && app.focus == Focus::Inspector {
        render_inspector(frame, app, main[1]);
    } else {
        render_reader(frame, app, main[1]);
    }
    if main.len() == 3 {
        render_inspector(frame, app, main[2]);
    }
    frame.render_widget(
        Paragraph::new(status_line(app)).style(Style::default().fg(Color::DarkGray)),
        vertical[1],
    );
    if let Some(overlay) = &app.overlay {
        render_overlay(frame, overlay, centered_rect(72, 60, area));
    }
}

fn status_line(app: &App) -> String {
    format!(
        "[{}] {}  ·  / search  i IC  v mode  b branches  t technical  y copy  ? help  q quit",
        app.focus.label(),
        app.status
    )
}

fn pane_block(title: String, focused: bool) -> Block<'static> {
    let mut block = Block::default().title(title).borders(Borders::ALL);
    if focused {
        block = block.border_style(Style::default().fg(Color::Cyan));
    }
    block
}

fn render_conversations(frame: &mut Frame, app: &App, area: Rect) {
    let title = app.conversations_panel_title();
    let title_width = usize::from(area.width.saturating_sub(4).max(12));
    let items: Vec<ListItem> = app
        .conversations
        .iter()
        .map(|conversation| {
            let label = if conversation.title.trim().is_empty() {
                "Untitled".to_string()
            } else {
                truncate(&conversation.title, title_width)
            };
            let line = if conversation.has_branches {
                Line::from(vec![
                    Span::styled("● ", Style::default().fg(Color::Yellow)),
                    Span::raw(label),
                ])
            } else {
                Line::from(vec![Span::raw(format!("  {label}"))])
            };
            ListItem::new(line).style(Style::default().fg(Color::DarkGray))
        })
        .collect();
    let list = List::new(items)
        .block(pane_block(title, app.focus == Focus::Conversations))
        .highlight_style(
            Style::default()
                .fg(Color::White)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = app.conversations_state;
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_reader(frame: &mut Frame, app: &mut App, area: Rect) {
    app.reader_wrap_width = area.width;
    let wrap_width = usize::from(area.width.saturating_sub(4).max(20));
    let title = app.reader_panel_title();
    let lines = if app.loading_conversation && app.messages.is_empty() {
        vec![Line::from("Loading conversation…")]
    } else {
        app.reader_lines(wrap_width)
    };
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(pane_block(title, app.focus == Focus::Reader))
            .scroll((app.reader_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn build_reader_lines(app: &App, wrap_width: usize) -> Vec<Line<'static>> {
    if app.messages.is_empty() {
        return vec![Line::from("No messages to display")];
    }
    let mut lines = Vec::new();
    let mut index = 0;
    while index < app.messages.len() {
        if !app.technical_visible && app.messages[index].is_technical() {
            let start = index;
            while index < app.messages.len() && app.messages[index].is_technical() {
                index += 1;
            }
            let count = index - start;
            let selected = (start..index).contains(&app.message_selected);
            let prefix = if selected { ">" } else { " " };
            let label = if count == 1 {
                "1 technical message hidden · press t".to_string()
            } else {
                format!("{count} technical messages hidden · press t")
            };
            lines.push(Line::from(format!("{prefix} ▸ {label}")));
            continue;
        }
        lines.extend(message_lines(app, index, wrap_width));
        index += 1;
    }
    lines
}

fn message_lines(app: &App, index: usize, wrap_width: usize) -> Vec<Line<'static>> {
    let message = &app.messages[index];
    let selected = index == app.message_selected;
    let prefix = if selected { ">" } else { " " };
    let ic = message
        .ic
        .map(|value| format!("[IC:{value}] "))
        .unwrap_or_default();
    let branch_hint = if selected {
        app.branch_alternatives
            .map(|count| format!(" · {count} alternatives · press b"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let heading = Line::from(vec![
        Span::styled(prefix, Style::default().fg(Color::Yellow)),
        Span::styled(
            format!(" {ic}{}", message.role.to_uppercase()),
            if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Green)
            },
        ),
        Span::raw(format!(
            " · {}{}",
            message.timestamp.as_deref().unwrap_or("unknown date"),
            branch_hint
        )),
    ]);
    let mut block = vec![heading];
    block.extend(
        wrap_content(&message.content, wrap_width)
            .into_iter()
            .map(Line::from),
    );
    block.push(Line::from(""));
    block
}

fn asset_status_label(asset: &AssetView) -> &'static str {
    if asset.exists_locally {
        "✓ local"
    } else if asset.relative_path.is_some() {
        "× missing"
    } else {
        "– not in export"
    }
}

fn render_inspector(frame: &mut Frame, app: &App, area: Rect) {
    let lines = if let Some(message) = app.selected_message() {
        let stable_reference = message
            .ic
            .and_then(|ic| MessageReference::new(ic, message.id.clone()).ok())
            .map(|reference| reference.human())
            .unwrap_or_else(|| "none (technical role)".into());
        let mut lines = vec![
            Line::from(format!("UUID: {}", truncate(&message.id, 24))),
            Line::from(format!("Stable reference: {stable_reference}")),
            Line::from(format!("Role: {}", message.role)),
            Line::from(format!("Node: {}", truncate(&message.node_id, 24))),
            Line::from(format!(
                "Parent: {}",
                message
                    .parent_node_id
                    .as_deref()
                    .map(|id| truncate(id, 20))
                    .unwrap_or_else(|| "root".into())
            )),
        ];
        if !app.assets.is_empty() {
            lines.push(Line::from("Assets:"));
            lines.extend(app.assets.iter().map(|asset| {
                Line::from(format!(
                    "{} {}",
                    asset_status_label(asset),
                    truncate(&asset.name, 22)
                ))
            }));
        }
        lines
    } else {
        vec![Line::from("Select a message to inspect")]
    };
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(pane_block(
                "Inspector".into(),
                app.focus == Focus::Inspector,
            ))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_overlay(frame: &mut Frame, overlay: &Overlay, area: Rect) {
    frame.render_widget(Clear, area);
    match overlay {
        Overlay::Search { value } => frame.render_widget(
            Paragraph::new(format!("Search: {value}\n\nEnter to search · Esc to cancel"))
                .block(Block::default().title("Search").borders(Borders::ALL)),
            area,
        ),
        Overlay::JumpIc { value } => frame.render_widget(
            Paragraph::new(format!(
                "Reference: {value}\n\nIC, msg:<id>, or composite reference · Enter to open · Esc to cancel"
            ))
            .block(Block::default().title("Jump to message").borders(Borders::ALL)),
            area,
        ),
        Overlay::Branches { choices, selected } => {
            let items = choices
                .iter()
                .map(|choice| {
                    let ic = choice.ic.map(|value| format!("[IC:{value}] ")).unwrap_or_default();
                    ListItem::new(format!("{ic}{} · {}", choice.role.as_deref().unwrap_or("node"), first_line(&choice.preview, 48)))
                })
                .collect::<Vec<_>>();
            let mut state = ListState::default();
            state.select(Some(*selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(Block::default().title("Branch alternatives").borders(Borders::ALL))
                    .highlight_style(Style::default().bg(Color::Blue)),
                area,
                &mut state,
            );
        }
        Overlay::Context { lines } => frame.render_widget(
            Paragraph::new(lines.join("\n"))
                .block(Block::default().title("IC context · Esc to close").borders(Borders::ALL))
                .wrap(Wrap { trim: true }),
            area,
        ),
        Overlay::SearchResults {
            query,
            hits,
            selected,
        } => {
            if hits.is_empty() {
                frame.render_widget(
                    Paragraph::new(format!("No results for “{query}”\n\nEsc to close"))
                        .block(Block::default().title("Search results").borders(Borders::ALL)),
                    area,
                );
                return;
            }
            let items = hits
                .iter()
                .map(|hit| {
                    let ic = hit
                        .ic
                        .map(|value| format!("[IC:{value}] "))
                        .unwrap_or_default();
                    ListItem::new(vec![
                        Line::from(format!(
                            "{ic}{} · {}",
                            hit.role.to_uppercase(),
                            hit.conversation_title
                        )),
                        Line::from(first_line(&hit.excerpt, 56))
                            .style(Style::default().fg(Color::DarkGray)),
                    ])
                })
                .collect::<Vec<_>>();
            let capped = hits.len() >= PAGE_SIZE;
            let overlay_title = if capped {
                format!("Search · “{query}” · {} shown · more may exist", hits.len())
            } else {
                format!("Search · “{query}” · Enter open")
            };
            let mut state = ListState::default();
            state.select(Some(*selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(Block::default().title(overlay_title).borders(Borders::ALL))
                    .highlight_style(Style::default().bg(Color::Blue)),
                area,
                &mut state,
            );
        }
        Overlay::Help => frame.render_widget(
            Paragraph::new(
                "RecallEngine browse — local mail-like reader\n\n\
                 Left: conversation titles only — j/k schedules loading after 180 ms\n\
                 Thread mode: root → export current_node branch; search/IC/branch switches active branch\n\
                 All messages: ascending IC order, capped at 500 when larger\n\n\
                 /  search overlay (never replaces conversation list)\n\
                 i  jump by IC, message ID, or composite reference\n\
                 Enter  load immediately when pending, then focus reader\n\
                 v  Thread / All messages\n\
                 b  branch alternatives (when shown on selected message)\n\
                 t  show/hide technical messages (grouped when hidden)\n\
                 y  copy [IC:n | msg:id] — status confirms or preserves reference on failure\n\
                 c  IC neighborhood context\n\
                 Tab  Conversations → Reader → Inspector\n\
                 Esc  resync sidebar after IC/search/branch jump\n\
                 ●  conversation has branches\n\
                 q  quit\n\n\
                 Layout medium (80–119 cols): Conversations + Reader, or Conversations + Inspector\n\
                 Layout compact (<80 cols): one pane at a time via Tab\n\n\
                 Esc or ? closes overlays",
            )
            .block(Block::default().title("RecallEngine browse help").borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
            area,
        ),
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn truncate(value: &str, max: usize) -> String {
    let mut characters = value.chars();
    let truncated: String = characters.by_ref().take(max).collect();
    if characters.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn first_line(value: &str, max: usize) -> String {
    truncate(value.lines().next().unwrap_or_default(), max)
}

fn wrap_content(content: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    for paragraph in content.lines() {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut start = 0;
        let chars: Vec<char> = paragraph.chars().collect();
        while start < chars.len() {
            let end = (start + width).min(chars.len());
            let mut slice_end = end;
            if end < chars.len() {
                while slice_end > start && !chars[slice_end - 1].is_whitespace() {
                    slice_end -= 1;
                }
                if slice_end == start {
                    slice_end = end;
                }
            }
            lines.push(chars[start..slice_end].iter().collect());
            start = slice_end;
            while start < chars.len() && chars[start].is_whitespace() {
                start += 1;
            }
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn parse_ic_query(query: &str) -> Option<i64> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    if let Some(value) = query
        .strip_prefix("[IC:")
        .and_then(|value| value.strip_suffix(']'))
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
    {
        return Some(value);
    }
    if let Some(value) = query
        .strip_prefix("IC:")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
    {
        return Some(value);
    }
    if query.chars().all(|ch| ch.is_ascii_digit()) {
        return query.parse::<i64>().ok().filter(|value| *value > 0);
    }
    None
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use crate::{cli::AssetMode, commands::import::run_chatgpt_import, read_model::ReadRepository};

    use super::{
        first_line, parse_ic_query, truncate, wrap_content, App, Focus, ReaderMode,
        ALL_MESSAGES_CAP, CONVERSATION_LOAD_DEBOUNCE, PAGE_SIZE, THREAD_CACHE_LIMIT,
    };

    #[test]
    fn truncates_on_character_boundaries() {
        assert_eq!(truncate("éclair", 2), "éc…");
        assert_eq!(first_line("one\ntwo", 10), "one");
    }

    #[test]
    fn labels_reader_modes() {
        assert_eq!(ReaderMode::Thread.label(), "Thread");
        assert_eq!(ReaderMode::AllMessages.label(), "All messages · IC order");
    }

    #[test]
    fn public_limits_are_stable() {
        assert_eq!(PAGE_SIZE, 150);
        assert_eq!(ALL_MESSAGES_CAP, 500);
        assert_eq!(THREAD_CACHE_LIMIT, 12);
        assert_eq!(CONVERSATION_LOAD_DEBOUNCE, Duration::from_millis(180));
    }

    #[test]
    fn wraps_long_lines_and_preserves_paragraphs() {
        let wrapped = wrap_content("hello world again", 5);
        assert_eq!(wrapped, vec!["hello", "world", "again"]);
        let multiline = wrap_content("line one\nline two", 20);
        assert_eq!(multiline, vec!["line one", "line two"]);
    }

    #[test]
    fn focus_labels_are_stable() {
        assert_eq!(Focus::Reader.label(), "Reader");
    }

    #[test]
    fn parse_ic_query_accepts_common_forms() {
        assert_eq!(parse_ic_query("42"), Some(42));
        assert_eq!(parse_ic_query("IC:42"), Some(42));
        assert_eq!(parse_ic_query("[IC:42]"), Some(42));
        assert_eq!(parse_ic_query("hello"), None);
        assert_eq!(parse_ic_query("0"), None);
    }

    #[test]
    fn debounce_keeps_current_conversation_and_loads_only_latest_selection() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("history.sqlite");
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
        run_chatgpt_import(fixture, db.clone(), AssetMode::External, None, false, None).unwrap();
        let mut app = App::new(ReadRepository::open_read_only(&db).unwrap()).unwrap();
        assert!(app.conversations.len() >= 2);

        app.conversation_nav_at -= CONVERSATION_LOAD_DEBOUNCE;
        app.flush_pending_conversation_load().unwrap();
        let first = app.conversation_id.clone().unwrap();
        let second = app.conversations[1].id.clone();

        app.schedule_conversation_load(first.clone());
        app.schedule_conversation_load(second.clone());
        app.flush_pending_conversation_load().unwrap();
        assert_eq!(app.conversation_id.as_deref(), Some(first.as_str()));
        assert_eq!(
            app.pending_conversation_id.as_deref(),
            Some(second.as_str())
        );

        app.conversation_nav_at -= CONVERSATION_LOAD_DEBOUNCE + Duration::from_millis(1);
        app.flush_pending_conversation_load().unwrap();
        assert_eq!(app.conversation_id.as_deref(), Some(second.as_str()));
        assert!(app.pending_conversation_id.is_none());
    }

    #[test]
    fn thread_cache_is_bounded_and_refreshes_lru_order_on_read() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("history.sqlite");
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
        run_chatgpt_import(fixture, db.clone(), AssetMode::External, None, false, None).unwrap();
        let mut app = App::new(ReadRepository::open_read_only(&db).unwrap()).unwrap();
        app.thread_cache.clear();
        app.thread_cache_order.clear();

        for index in 0..12 {
            app.store_thread_cache_entry(format!("key-{index}"), &[]);
        }
        assert!(app.cached_thread_entry("key-0").is_some());
        app.store_thread_cache_entry("key-12".into(), &[]);

        assert_eq!(app.thread_cache.len(), 12);
        assert!(app.thread_cache.contains_key("key-0"));
        assert!(!app.thread_cache.contains_key("key-1"));
    }

    #[test]
    fn asset_status_labels_are_distinct() {
        use crate::read_model::AssetView;
        let local = AssetView {
            id: "1".into(),
            name: "a".into(),
            mime_type: None,
            exists_locally: true,
            relative_path: Some("assets/a".into()),
        };
        let missing = AssetView {
            id: "2".into(),
            name: "b".into(),
            mime_type: None,
            exists_locally: false,
            relative_path: Some("assets/b".into()),
        };
        let absent = AssetView {
            id: "3".into(),
            name: "c".into(),
            mime_type: None,
            exists_locally: false,
            relative_path: None,
        };
        assert_eq!(super::asset_status_label(&local), "✓ local");
        assert_eq!(super::asset_status_label(&missing), "× missing");
        assert_eq!(super::asset_status_label(&absent), "– not in export");
    }
}
