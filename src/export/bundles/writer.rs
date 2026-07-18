#![allow(clippy::type_complexity)]

use crate::export::bundles::models::{BundleFilePlan, BundlePlan};
use crate::export::markdown::renderer::MarkdownRenderer;
use crate::models::{ContentBlock, Conversation, Message};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;
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
        force: bool,
    ) -> Result<(), io::Error> {
        let parent = parent_dir(out_dir);
        fs::create_dir_all(&parent)?;

        if out_dir.exists() && !force {
            return Err(destination_exists(out_dir));
        }

        let tmp = tempfile::Builder::new()
            .prefix("recall-bundle-")
            .tempdir_in(&parent)?;
        let tmp_path = tmp.path().to_path_buf();

        let write_result: Result<(), io::Error> = (|| {
            self.write_manifest(plan, &tmp_path)?;
            for quarter in &plan.quarters {
                for file_plan in &quarter.files {
                    let file_path = tmp_path.join(&file_plan.relative_path);
                    if let Some(file_parent) = file_path.parent() {
                        fs::create_dir_all(file_parent)?;
                    }
                    let mut file = File::create(&file_path)?;
                    self.write_file_content(&mut file, file_plan, conversations_map)?;
                }
            }
            Ok(())
        })();
        // On failure, TempDir cleans up on drop and the destination stays untouched.
        write_result?;

        let staged = tmp.keep();
        publish_path(&staged, out_dir, force, PublishKind::Directory).inspect_err(|_| {
            let _ = fs::remove_dir_all(&staged);
        })?;
        Ok(())
    }

    pub fn write_zip(
        &self,
        plan: &BundlePlan,
        conversations_map: &HashMap<String, &(Conversation, Vec<(Message, Vec<ContentBlock>)>)>,
        out_zip: &Path,
        force: bool,
    ) -> Result<(), io::Error> {
        let parent = parent_dir(out_zip);
        fs::create_dir_all(&parent)?;

        if out_zip.exists() && !force {
            return Err(destination_exists(out_zip));
        }

        let mut tmp = tempfile::Builder::new()
            .prefix("recall-bundle-")
            .suffix(".zip")
            .tempfile_in(&parent)?;

        {
            let mut zip = ZipWriter::new(tmp.as_file_mut());
            let time = zip::DateTime::from_date_and_time(2023, 1, 1, 0, 0, 0).unwrap_or_default();
            let options = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(0o644)
                .last_modified_time(time);

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
        }

        let staged = tmp.into_temp_path();
        match publish_path(&staged, out_zip, force, PublishKind::File) {
            Ok(()) => {
                // File was renamed into place; disable cleanup of the old temp path.
                let _ = staged.keep();
                Ok(())
            }
            Err(error) => {
                // TempPath removes the staged file on drop.
                Err(error)
            }
        }
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
                    if let Some((msg, blocks)) =
                        messages.iter().find(|(m, _)| m.id == slice.message_id)
                    {
                        if let Some(env) = self.renderer.render_message_envelope(msg) {
                            writer.write_all(env.as_bytes())?;
                        }

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

#[derive(Clone, Copy)]
enum PublishKind {
    Directory,
    File,
}

fn parent_dir(path: &Path) -> PathBuf {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn destination_exists(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "refusing to overwrite existing destination {} (pass --force to replace)",
            path.display()
        ),
    )
}

/// Publish a staged path into `destination` without deleting the destination first.
///
/// On replacement, the existing destination is renamed aside first. Only that backup
/// (created by this function) may be removed after a successful swap.
fn publish_path(
    staged: &Path,
    destination: &Path,
    force: bool,
    kind: PublishKind,
) -> Result<(), io::Error> {
    if destination.exists() {
        if !force {
            return Err(destination_exists(destination));
        }
        let parent = parent_dir(destination);
        let backup = unique_backup_path(&parent, destination, kind);
        fs::rename(destination, &backup)?;
        match fs::rename(staged, destination) {
            Ok(()) => {
                match kind {
                    PublishKind::Directory => {
                        let _ = fs::remove_dir_all(&backup);
                    }
                    PublishKind::File => {
                        let _ = fs::remove_file(&backup);
                    }
                }
                Ok(())
            }
            Err(error) => {
                let _ = fs::rename(&backup, destination);
                Err(error)
            }
        }
    } else {
        fs::rename(staged, destination)
    }
}

fn unique_backup_path(parent: &Path, destination: &Path, kind: PublishKind) -> PathBuf {
    let stem = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("bundle");
    match kind {
        PublishKind::Directory => {
            parent.join(format!(".recall-bundle-backup-{stem}-{}", Uuid::new_v4()))
        }
        PublishKind::File => parent.join(format!(
            ".recall-bundle-backup-{stem}-{}.tmp",
            Uuid::new_v4()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::bundles::models::BundleStatistics;
    use crate::export::bundles::profile::BundleProfile;
    use crate::export::markdown::options::MarkdownRenderOptions;

    fn empty_plan() -> BundlePlan {
        BundlePlan {
            profile: BundleProfile::notebooklm(),
            quarters: Vec::new(),
            statistics: BundleStatistics {
                total_conversations: 0,
                total_messages: 0,
                total_words: 0,
                total_bytes: 0,
                sharded_conversations: 0,
                content_files_count: 0,
            },
        }
    }

    #[test]
    fn refuses_existing_directory_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("bundle-out");
        fs::create_dir_all(&out).unwrap();
        let marker = out.join("do-not-delete.txt");
        fs::write(&marker, "keep-me").unwrap();
        let sibling = tmp.path().join("sibling");
        fs::create_dir_all(&sibling).unwrap();
        fs::write(sibling.join("safe.txt"), "safe").unwrap();

        let options = MarkdownRenderOptions::default();
        let renderer = MarkdownRenderer::new(&options);
        let writer = BundleWriter::new(&renderer);
        let plan = empty_plan();
        let map = HashMap::new();

        let err = writer
            .write_directory(&plan, &map, &out, false)
            .expect_err("must refuse existing destination");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(err.to_string().contains("--force"));
        assert_eq!(fs::read_to_string(&marker).unwrap(), "keep-me");
        assert_eq!(
            fs::read_to_string(sibling.join("safe.txt")).unwrap(),
            "safe"
        );
    }

    #[test]
    fn force_replaces_directory_without_touching_siblings() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("bundle-out");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("old.txt"), "old").unwrap();
        let sibling = tmp.path().join("sibling");
        fs::create_dir_all(&sibling).unwrap();
        fs::write(sibling.join("safe.txt"), "safe").unwrap();

        let options = MarkdownRenderOptions::default();
        let renderer = MarkdownRenderer::new(&options);
        let writer = BundleWriter::new(&renderer);
        let plan = empty_plan();
        let map = HashMap::new();

        writer
            .write_directory(&plan, &map, &out, true)
            .expect("force write");
        assert!(out.join("manifest.json").is_file());
        assert!(!out.join("old.txt").exists());
        assert_eq!(
            fs::read_to_string(sibling.join("safe.txt")).unwrap(),
            "safe"
        );
        // No leftover user-named *.tmp directory next to the destination.
        assert!(!out.with_extension("tmp").exists());
    }

    #[test]
    fn refuses_existing_zip_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("bundle.zip");
        fs::write(&out, "old-zip").unwrap();

        let options = MarkdownRenderOptions::default();
        let renderer = MarkdownRenderer::new(&options);
        let writer = BundleWriter::new(&renderer);
        let plan = empty_plan();
        let map = HashMap::new();

        let err = writer
            .write_zip(&plan, &map, &out, false)
            .expect_err("must refuse existing zip");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&out).unwrap(), b"old-zip");
    }

    #[test]
    fn force_replaces_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("bundle.zip");
        fs::write(&out, "old-zip").unwrap();

        let options = MarkdownRenderOptions::default();
        let renderer = MarkdownRenderer::new(&options);
        let writer = BundleWriter::new(&renderer);
        let plan = empty_plan();
        let map = HashMap::new();

        writer
            .write_zip(&plan, &map, &out, true)
            .expect("force zip");
        let bytes = fs::read(&out).unwrap();
        assert_ne!(bytes, b"old-zip");
        assert!(bytes.starts_with(b"PK"));
    }

    #[test]
    fn writes_fresh_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("fresh-bundle");

        let options = MarkdownRenderOptions::default();
        let renderer = MarkdownRenderer::new(&options);
        let writer = BundleWriter::new(&renderer);
        let plan = empty_plan();
        let map = HashMap::new();

        writer
            .write_directory(&plan, &map, &out, false)
            .expect("fresh write");
        assert!(out.join("manifest.json").is_file());
    }
}
