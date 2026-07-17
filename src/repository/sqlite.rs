use crate::error::Result;
use crate::models::{ContentBlock, Conversation, Message};
use rusqlite::{params, Connection, OptionalExtension};

pub struct SqliteRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn get_conversation(&self, id: &str) -> Result<Option<Conversation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, create_time, update_time, current_node_id, 
                    default_model_slug, is_archived, is_starred, source_relative_path 
             FROM conversations WHERE id = ?",
        )?;

        let conv = stmt
            .query_row(params![id], |row| {
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    create_time: row.get(2)?,
                    update_time: row.get(3)?,
                    current_node_id: row.get(4)?,
                    default_model_slug: row.get(5)?,
                    is_archived: row.get::<_, i32>(6)? != 0,
                    is_starred: row.get::<_, i32>(7)? != 0,
                    source_relative_path: row.get(8)?,
                })
            })
            .optional()?;

        Ok(conv)
    }

    pub fn list_conversations(&self) -> Result<Vec<Conversation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, create_time, update_time, current_node_id, 
                    default_model_slug, is_archived, is_starred, source_relative_path 
             FROM conversations 
             ORDER BY create_time ASC, id ASC",
        )?;

        let iter = stmt.query_map([], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                create_time: row.get(2)?,
                update_time: row.get(3)?,
                current_node_id: row.get(4)?,
                default_model_slug: row.get(5)?,
                is_archived: row.get::<_, i32>(6)? != 0,
                is_starred: row.get::<_, i32>(7)? != 0,
                source_relative_path: row.get(8)?,
            })
        })?;

        let mut convs = Vec::new();
        for conv in iter {
            convs.push(conv?);
        }

        Ok(convs)
    }

    pub fn get_messages(&self, conversation_id: &str) -> Result<Vec<Message>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ic, node_id, conversation_id, role, author_name, 
                    create_time, timestamp, source_shard_index, source_node_order, 
                    model_slug, content_type, is_active 
             FROM messages 
             WHERE conversation_id = ? 
             ORDER BY source_node_order ASC, create_time ASC, id ASC",
        )?;

        let iter = stmt.query_map(params![conversation_id], |row| {
            Ok(Message {
                id: row.get(0)?,
                ic: row.get(1)?,
                node_id: row.get(2)?,
                conversation_id: row.get(3)?,
                role: row.get(4)?,
                author_name: row.get(5)?,
                create_time: row.get(6)?,
                timestamp: row.get(7)?,
                source_shard_index: row.get(8)?,
                source_node_order: row.get(9)?,
                model_slug: row.get(10)?,
                content_type: row.get(11)?,
                is_active: row.get::<_, i32>(12)? != 0,
            })
        })?;

        let mut messages = Vec::new();
        for msg in iter {
            messages.push(msg?);
        }

        Ok(messages)
    }

    pub fn get_content_blocks(&self, message_id: &str) -> Result<Vec<ContentBlock>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, message_id, ordinal, kind, text_content, json_content 
             FROM content_blocks 
             WHERE message_id = ? 
             ORDER BY ordinal ASC",
        )?;

        let iter = stmt.query_map(params![message_id], |row| {
            Ok(ContentBlock {
                id: row.get(0)?,
                message_id: row.get(1)?,
                ordinal: row.get(2)?,
                kind: row.get(3)?,
                text_content: row.get(4)?,
                json_content: row.get(5)?,
            })
        })?;

        let mut blocks = Vec::new();
        for block in iter {
            blocks.push(block?);
        }

        Ok(blocks)
    }

    pub fn resolve_ic(&self, ic: i64) -> Result<Option<Conversation>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.title, c.create_time, c.update_time, c.current_node_id, 
                    c.default_model_slug, c.is_archived, c.is_starred, c.source_relative_path 
             FROM messages m
             JOIN conversations c ON m.conversation_id = c.id
             WHERE m.ic = ? LIMIT 1",
        )?;

        let conv = stmt
            .query_row(params![ic], |row| {
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    create_time: row.get(2)?,
                    update_time: row.get(3)?,
                    current_node_id: row.get(4)?,
                    default_model_slug: row.get(5)?,
                    is_archived: row.get::<_, i32>(6)? != 0,
                    is_starred: row.get::<_, i32>(7)? != 0,
                    source_relative_path: row.get(8)?,
                })
            })
            .optional()?;

        Ok(conv)
    }
}
