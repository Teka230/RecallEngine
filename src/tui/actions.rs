use std::time::Instant;

use crate::{domain::reference::MessageReference, read_model::IcJumpTarget, Result};

use super::state::{
    App, Focus, InputMode, Overlay, ReaderMode, ALL_MESSAGES_CAP, CONVERSATION_LOAD_DEBOUNCE,
    PAGE_SIZE,
};
use super::text::{first_line, parse_ic_query};

impl App {
    pub fn schedule_conversation_load(&mut self, conversation_id: String) {
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

    pub fn flush_pending_conversation_load(&mut self) -> Result<()> {
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

    pub fn sync_sidebar_to_active(&mut self) -> Result<()> {
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

    pub fn jump_sidebar_to_index(&mut self, index: usize, conversation_id: &str) -> Result<()> {
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

    pub fn load_conversation(&mut self, conversation_id: &str, focus_reader: bool) -> Result<()> {
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

    pub fn open_ic(&mut self, ic: i64) -> Result<()> {
        let Some(target) = self.repository.resolve_ic_jump(ic)? else {
            self.status = format!("IC {ic} not found");
            return Ok(());
        };
        self.open_jump_target(target, format!("IC {ic}"))
    }

    pub fn open_message_id(&mut self, message_id: &str) -> Result<()> {
        let Some(target) = self.repository.resolve_message_id_jump(message_id)? else {
            self.status = format!("Message {message_id} not found");
            return Ok(());
        };
        self.open_jump_target(target, format!("msg:{message_id}"))
    }

    pub fn open_reference(&mut self, reference: &MessageReference) -> Result<()> {
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

    pub fn open_jump_target(&mut self, target: IcJumpTarget, label: String) -> Result<()> {
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

    pub fn show_first_message(&mut self) -> Result<()> {
        self.message_selected = 0;
        self.reader_scroll = 0;
        self.refresh_inspector()?;
        Ok(())
    }

    pub fn move_message(&mut self, delta: isize) -> Result<()> {
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

    pub fn load_more_conversations(&mut self) -> Result<()> {
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

    pub fn move_conversation(&mut self, delta: isize) -> Result<()> {
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

    pub fn open_search_hit(&mut self, message_id: &str) -> Result<()> {
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

    pub fn toggle_mode(&mut self) -> Result<()> {
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

    pub fn open_branches(&mut self) -> Result<()> {
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

    pub fn copy_citation(&mut self) {
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

    pub fn open_context(&mut self) -> Result<()> {
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

    pub fn start_search(&mut self) {
        self.input_mode = InputMode::Search;
        self.focus = Focus::Input;
    }

    pub fn start_ic_jump(&mut self) {
        self.input_mode = InputMode::Jump;
        self.focus = Focus::Input;
    }

    pub fn submit_search(&mut self, query: String) -> Result<()> {
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

    pub fn open_reference_input(&mut self, value: &str, allow_raw_message_id: bool) -> Result<()> {
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
}
