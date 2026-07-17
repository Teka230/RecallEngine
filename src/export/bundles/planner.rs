#![allow(clippy::type_complexity)]
use crate::export::bundles::models::*;
use crate::export::bundles::profile::{BundleLimits, BundleProfile};
use crate::export::markdown::renderer::MarkdownRenderer;
use crate::models::{ContentBlock, Conversation, Message};
use chrono::{Datelike, TimeZone, Utc};
use std::collections::HashMap;

pub struct BundlePlanner<'a> {
    profile: BundleProfile,
    renderer: &'a MarkdownRenderer<'a>,
}

impl<'a> BundlePlanner<'a> {
    pub fn new(profile: BundleProfile, renderer: &'a MarkdownRenderer<'a>) -> Self {
        Self { profile, renderer }
    }

    pub fn plan(
        &self,
        conversations: Vec<(Conversation, Vec<(Message, Vec<ContentBlock>)>)>,
    ) -> Result<BundlePlan, String> {
        let mut stats = BundleStatistics {
            total_conversations: 0,
            total_messages: 0,
            total_words: 0,
            total_bytes: 0,
            sharded_conversations: 0,
            content_files_count: 0,
        };

        let mut quarters_map: HashMap<String, Vec<BundleFilePlan>> = HashMap::new();
        let limits = &self.profile.limits;

        for (conv, messages) in conversations {
            let quarter = self.get_quarter(&conv, &messages);

            let shards = self.shard_conversation(&conv, &messages, limits);

            if shards.len() > 1 {
                stats.sharded_conversations += 1;
            }
            stats.total_conversations += 1;

            for shard in shards {
                stats.total_messages += shard.messages_count;
                stats.total_words += shard.words;
                stats.total_bytes += shard.bytes;

                let files_for_quarter = quarters_map.entry(quarter.clone()).or_default();

                let mut placed = false;
                if let Some(last_file) = files_for_quarter.last_mut() {
                    if self.can_fit(last_file, &shard, limits) {
                        last_file.shards.push(shard.clone());
                        last_file.total_words += shard.words;
                        last_file.total_bytes += shard.bytes;
                        last_file.total_messages += shard.messages_count;
                        placed = true;
                    }
                }

                if !placed {
                    let file_index = files_for_quarter.len() + 1;
                    files_for_quarter.push(BundleFilePlan {
                        relative_path: format!("{}/file_{:03}.md", quarter, file_index),
                        shards: vec![shard.clone()],
                        total_words: shard.words,
                        total_bytes: shard.bytes,
                        total_messages: shard.messages_count,
                    });
                    stats.content_files_count += 1;
                }
            }
        }

        let mut quarters: Vec<QuarterPlan> = quarters_map
            .into_iter()
            .map(|(name, files)| QuarterPlan { name, files })
            .collect();

        quarters.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(BundlePlan {
            profile: self.profile.clone(),
            quarters,
            statistics: stats,
        })
    }

    fn can_fit(&self, file: &BundleFilePlan, shard: &ShardPlan, limits: &BundleLimits) -> bool {
        if let Some(mw) = limits.max_words_per_file {
            if file.total_words + shard.words > mw {
                return false;
            }
        }
        if let Some(mb) = limits.max_bytes_per_file {
            if file.total_bytes + shard.bytes > mb {
                return false;
            }
        }
        if let Some(mc) = limits.max_conversations_per_file {
            if file.shards.len() as u32 + 1 > mc {
                return false;
            }
        }
        true
    }

    fn shard_conversation(
        &self,
        conv: &Conversation,
        messages: &[(Message, Vec<ContentBlock>)],
        limits: &BundleLimits,
    ) -> Vec<ShardPlan> {
        let mut shards = Vec::new();
        let max_b = limits.max_bytes_per_file.unwrap_or(u64::MAX);
        let max_w = limits.max_words_per_file.unwrap_or(u64::MAX);

        let mut part_number = 1;

        let reset_current = |conv: &Conversation,
                             part_number: u32,
                             renderer: &MarkdownRenderer|
         -> (u64, u64, Vec<MessageSlicePlan>, usize) {
            let header =
                renderer.render_conversation_header(conv, Some((part_number, part_number)));
            (
                MarkdownRenderer::count_bytes(&header),
                MarkdownRenderer::count_words(&header),
                Vec::new(),
                0,
            )
        };

        let (mut current_bytes, mut current_words, mut current_slices, mut current_msg_count) =
            reset_current(conv, part_number, self.renderer);

        for (msg, blocks) in messages {
            let env_str = self.renderer.render_message_envelope(msg);
            if env_str.is_none() {
                continue;
            }
            let env_str = env_str.unwrap();
            let env_b = MarkdownRenderer::count_bytes(&env_str);
            let env_w = MarkdownRenderer::count_words(&env_str);

            let mut block_slices = Vec::new();
            let mut current_msg_b = env_b;
            let mut current_msg_w = env_w;

            for block in blocks {
                if !self.renderer.has_content(block) {
                    continue;
                }

                let block_str = self
                    .renderer
                    .render_block_content(block, 0, usize::MAX)
                    .unwrap_or_default();
                let block_b = MarkdownRenderer::count_bytes(&block_str);
                let block_w = MarkdownRenderer::count_words(&block_str);

                if (!current_slices.is_empty() || !block_slices.is_empty())
                    && (current_bytes + current_msg_b + block_b > max_b
                        || current_words + current_msg_w + block_w > max_w)
                {
                    if block_slices.is_empty() {
                        shards.push(self.create_shard(
                            conv,
                            part_number,
                            current_slices,
                            current_bytes,
                            current_words,
                            current_msg_count,
                        ));
                        part_number += 1;
                        let (cb, cw, c_slices, c_mc) =
                            reset_current(conv, part_number, self.renderer);
                        current_bytes = cb;
                        current_words = cw;
                        current_slices = c_slices;
                        current_msg_count = c_mc;

                        if current_bytes + current_msg_b + block_b <= max_b
                            && current_words + current_msg_w + block_w <= max_w
                        {
                            block_slices.push(BlockSlicePlan {
                                ordinal: block.ordinal,
                                byte_start: 0,
                                byte_end: usize::MAX,
                            });
                            current_msg_b += block_b;
                            current_msg_w += block_w;
                            continue;
                        }
                    } else {
                        current_slices.push(MessageSlicePlan {
                            message_id: msg.id.clone(),
                            ic: msg.ic,
                            block_ranges: std::mem::take(&mut block_slices),
                        });
                        current_bytes += current_msg_b;
                        current_words += current_msg_w;
                        current_msg_count += 1;

                        shards.push(self.create_shard(
                            conv,
                            part_number,
                            current_slices,
                            current_bytes,
                            current_words,
                            current_msg_count,
                        ));
                        part_number += 1;
                        let (cb, cw, c_slices, c_mc) =
                            reset_current(conv, part_number, self.renderer);
                        current_bytes = cb;
                        current_words = cw;
                        current_slices = c_slices;
                        current_msg_count = c_mc;

                        current_msg_b = env_b;
                        current_msg_w = env_w;

                        if current_bytes + current_msg_b + block_b <= max_b
                            && current_words + current_msg_w + block_w <= max_w
                        {
                            block_slices.push(BlockSlicePlan {
                                ordinal: block.ordinal,
                                byte_start: 0,
                                byte_end: usize::MAX,
                            });
                            current_msg_b += block_b;
                            current_msg_w += block_w;
                            continue;
                        }
                    }
                }

                if current_bytes + current_msg_b + block_b > max_b
                    || current_words + current_msg_w + block_w > max_w
                {
                    let text = block.text_content.as_deref().unwrap_or("");
                    let mut byte_start = 0;
                    while byte_start < text.len() {
                        let mut byte_end = text.len();
                        let remaining_b = max_b.saturating_sub(current_bytes + current_msg_b);
                        if remaining_b < block_b {
                            byte_end = byte_start + remaining_b as usize;
                            if byte_end >= text.len() {
                                byte_end = text.len();
                            } else {
                                while byte_end > byte_start && !text.is_char_boundary(byte_end) {
                                    byte_end -= 1;
                                }
                            }
                            if byte_end == byte_start {
                                if remaining_b == 0 {
                                    if !block_slices.is_empty() {
                                        current_slices.push(MessageSlicePlan {
                                            message_id: msg.id.clone(),
                                            ic: msg.ic,
                                            block_ranges: std::mem::take(&mut block_slices),
                                        });
                                        current_bytes += current_msg_b;
                                        current_words += current_msg_w;
                                        current_msg_count += 1;
                                    }
                                    if !current_slices.is_empty() {
                                        shards.push(self.create_shard(
                                            conv,
                                            part_number,
                                            current_slices,
                                            current_bytes,
                                            current_words,
                                            current_msg_count,
                                        ));
                                        part_number += 1;
                                    }
                                    let (cb, cw, c_slices, c_mc) =
                                        reset_current(conv, part_number, self.renderer);
                                    current_bytes = cb;
                                    current_words = cw;
                                    current_slices = c_slices;
                                    current_msg_count = c_mc;
                                    current_msg_b = env_b;
                                    current_msg_w = env_w;
                                    continue;
                                }
                                byte_end += 1;
                                while byte_end <= text.len() && !text.is_char_boundary(byte_end) {
                                    byte_end += 1;
                                }
                            }
                        }

                        let slice_str = self
                            .renderer
                            .render_block_content(block, byte_start, byte_end)
                            .unwrap_or_default();
                        let slice_b = MarkdownRenderer::count_bytes(&slice_str);
                        let slice_w = MarkdownRenderer::count_words(&slice_str);

                        block_slices.push(BlockSlicePlan {
                            ordinal: block.ordinal,
                            byte_start,
                            byte_end,
                        });
                        current_msg_b += slice_b;
                        current_msg_w += slice_w;
                        byte_start = byte_end;

                        if byte_start < text.len() {
                            current_slices.push(MessageSlicePlan {
                                message_id: msg.id.clone(),
                                ic: msg.ic,
                                block_ranges: std::mem::take(&mut block_slices),
                            });
                            current_bytes += current_msg_b;
                            current_words += current_msg_w;
                            current_msg_count += 1;

                            shards.push(self.create_shard(
                                conv,
                                part_number,
                                current_slices,
                                current_bytes,
                                current_words,
                                current_msg_count,
                            ));
                            part_number += 1;
                            let (cb, cw, c_slices, c_mc) =
                                reset_current(conv, part_number, self.renderer);
                            current_bytes = cb;
                            current_words = cw;
                            current_slices = c_slices;
                            current_msg_count = c_mc;
                            current_msg_b = env_b;
                            current_msg_w = env_w;
                        }
                    }
                } else {
                    block_slices.push(BlockSlicePlan {
                        ordinal: block.ordinal,
                        byte_start: 0,
                        byte_end: usize::MAX,
                    });
                    current_msg_b += block_b;
                    current_msg_w += block_w;
                }
            }

            if !block_slices.is_empty() {
                current_slices.push(MessageSlicePlan {
                    message_id: msg.id.clone(),
                    ic: msg.ic,
                    block_ranges: block_slices,
                });
                current_bytes += current_msg_b;
                current_words += current_msg_w;
                current_msg_count += 1;
            }
        }

        if !current_slices.is_empty() {
            shards.push(self.create_shard(
                conv,
                part_number,
                current_slices,
                current_bytes,
                current_words,
                current_msg_count,
            ));
        }

        let total_parts = shards.len() as u32;
        for shard in &mut shards {
            shard.total_parts = total_parts;
        }

        shards
    }

    fn create_shard(
        &self,
        conv: &Conversation,
        part_number: u32,
        slices: Vec<MessageSlicePlan>,
        bytes: u64,
        words: u64,
        messages_count: usize,
    ) -> ShardPlan {
        let first_ic = slices.first().map(|s| s.ic);
        let last_ic = slices.last().map(|s| s.ic);
        ShardPlan {
            conversation_id: conv.id.clone(),
            title: conv.title.clone().unwrap_or_else(|| "Untitled".to_string()),
            part_number,
            total_parts: part_number,
            message_slices: slices,
            first_ic,
            last_ic,
            words,
            bytes,
            messages_count,
        }
    }

    fn get_quarter(
        &self,
        conv: &Conversation,
        messages: &[(Message, Vec<ContentBlock>)],
    ) -> String {
        for (msg, blocks) in messages {
            if self.renderer.render_message(msg, blocks).is_some() {
                if let Some(create_time) = msg.create_time {
                    if let Some(dt) = Utc.timestamp_opt(create_time as i64, 0).single() {
                        let year = dt.year();
                        let quarter = (dt.month() - 1) / 3 + 1;
                        return format!("{}-Q{}", year, quarter);
                    }
                }
            }
        }
        if let Some(create_time) = conv.create_time {
            if let Some(dt) = Utc.timestamp_opt(create_time as i64, 0).single() {
                let year = dt.year();
                let quarter = (dt.month() - 1) / 3 + 1;
                return format!("{}-Q{}", year, quarter);
            }
        }
        "unknown-date".to_string()
    }
}
