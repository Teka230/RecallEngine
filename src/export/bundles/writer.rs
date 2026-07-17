#![allow(clippy::type_complexity)]

use crate::export::bundles::models::{BundleFilePlan, BundlePlan};
use crate::export::markdown::renderer::MarkdownRenderer;
use crate::models::{ContentBlock, Conversation, Message};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

pub struct BundleWriter<'a> {
    renderer: &'a MarkdownRenderer<'a>,
}

impl<'a> BundleWriter<'a> {
    pub fn new(renderer: &'a MarkdownRenderer<'a>) -> Self {
        Self { renderer }
    }

    pub fn write_directory(
        &self,
        plan: &BundlePlan,
        conversations_map: &HashMap<String, &(Conversation, Vec<(Message, Vec<ContentBlock>)>)>,
        out_dir: &Path,
    ) -> Result<(), io::Error> {
        let tmp_dir = out_dir.with_extension("tmp");
        if tmp_dir.exists() {
            fs::remove_dir_all(&tmp_dir)?;
        }
        fs::create_dir_all(&tmp_dir)?;

        self.write_manifest(plan, &tmp_dir)?;

        for quarter in &plan.quarters {
            for file_plan in &quarter.files {
                let file_path = tmp_dir.join(&file_plan.relative_path);
                if let Some(parent) = file_path.parent() {
                    fs::create_dir_all(parent)?;
                }

                let mut f = File::create(&file_path)?;
                self.write_file_content(&mut f, file_plan, conversations_map)?;
            }
        }

        // Atomic swap
        if out_dir.exists() {
            fs::remove_dir_all(out_dir)?;
        }
        fs::rename(&tmp_dir, out_dir)?;

        Ok(())
    }

    pub fn write_zip(
        &self,
        plan: &BundlePlan,
        conversations_map: &HashMap<String, &(Conversation, Vec<(Message, Vec<ContentBlock>)>)>,
        out_zip: &Path,
    ) -> Result<(), io::Error> {
        let tmp_zip = out_zip.with_extension("tmp");
        let file = File::create(&tmp_zip)?;
        let mut zip = ZipWriter::new(file);

        let time = zip::DateTime::from_date_and_time(2023, 1, 1, 0, 0, 0).unwrap_or_default();
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644)
            .last_modified_time(time);

        // Write manifest
        zip.start_file("manifest.json", options)?;
        let manifest_json = serde_json::to_string_pretty(&plan).unwrap_or_default();
        zip.write_all(manifest_json.as_bytes())?;

        for quarter in &plan.quarters {
            for file_plan in &quarter.files {
                zip.start_file(&file_plan.relative_path, options)?;
                self.write_file_content(&mut zip, file_plan, conversations_map)?;
            }
        }

        zip.finish()?;

        // Atomic swap
        if out_zip.exists() {
            fs::remove_file(out_zip)?;
        }
        fs::rename(&tmp_zip, out_zip)?;

        Ok(())
    }

    fn write_manifest(&self, plan: &BundlePlan, dir: &Path) -> Result<(), io::Error> {
        let manifest_path = dir.join("manifest.json");
        let manifest_json = serde_json::to_string_pretty(&plan).unwrap_or_default();
        fs::write(manifest_path, manifest_json)
    }

    fn write_file_content<W: Write>(
        &self,
        writer: &mut W,
        file_plan: &BundleFilePlan,
        conversations_map: &HashMap<String, &(Conversation, Vec<(Message, Vec<ContentBlock>)>)>,
    ) -> Result<(), io::Error> {
        for shard in &file_plan.shards {
            if let Some((conv, messages)) = conversations_map.get(&shard.conversation_id) {
                let header = self
                    .renderer
                    .render_conversation_header(conv, Some((shard.part_number, shard.total_parts)));
                writer.write_all(header.as_bytes())?;

                for slice in &shard.message_slices {
                    // Find the message
                    if let Some((msg, blocks)) =
                        messages.iter().find(|(m, _)| m.id == slice.message_id)
                    {
                        if let Some(env) = self.renderer.render_message_envelope(msg) {
                            writer.write_all(env.as_bytes())?;
                        }

                        // We render only the requested block slices
                        for block_slice in &slice.block_ranges {
                            if let Some(block) =
                                blocks.iter().find(|b| b.ordinal == block_slice.ordinal)
                            {
                                if let Some(rendered_block) = self.renderer.render_block_content(
                                    block,
                                    block_slice.byte_start,
                                    block_slice.byte_end,
                                ) {
                                    writer.write_all(rendered_block.as_bytes())?;
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
