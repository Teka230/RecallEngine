use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::domain::reference::ContextScope;

#[derive(Debug, Parser)]
#[command(
    name = "recall",
    about = "RecallEngine — ChatGPT export to canonical SQLite"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Import {
        #[command(subcommand)]
        target: ImportTarget,
    },
    Verify {
        #[arg(long)]
        db: PathBuf,
    },
    Stats {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Show {
        #[arg(long)]
        db: PathBuf,
        #[arg(
            long,
            required_unless_present_any = ["message_id", "reference"],
            conflicts_with_all = ["message_id", "reference"]
        )]
        ic: Option<i64>,
        #[arg(long, conflicts_with_all = ["ic", "reference"])]
        message_id: Option<String>,
        #[arg(long, conflicts_with_all = ["ic", "message_id"])]
        reference: Option<String>,
        #[arg(long)]
        before: Option<usize>,
        #[arg(long)]
        after: Option<usize>,
        #[arg(long, value_enum, default_value = "conversation")]
        scope: ContextScope,
        #[arg(long)]
        json: bool,
    },
    Browse {
        #[arg(long)]
        db: PathBuf,
        #[arg(long, conflicts_with = "conversation")]
        ic: Option<i64>,
        #[arg(long, conflicts_with = "ic")]
        conversation: Option<String>,
    },
    Serve {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        assets_dir: Option<PathBuf>,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 8788)]
        port: u16,
    },
    Export {
        #[command(subcommand)]
        target: ExportTarget,
    },
}

#[derive(Debug, Subcommand)]
pub enum ImportTarget {
    Chatgpt {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        db: PathBuf,
        #[arg(long, default_value = "external")]
        assets: AssetMode,
        #[arg(long)]
        assets_dir: Option<PathBuf>,
        #[arg(long)]
        strict: bool,
        #[arg(long)]
        seed_legacy_ic: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum AssetMode {
    External,
    Copy,
    Symlink,
}

#[derive(Debug, Subcommand)]
pub enum ExportTarget {
    LegacySqlite {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Cli;

    #[test]
    fn show_accepts_exactly_one_reference_form() {
        assert!(Cli::try_parse_from(["recall", "show", "--db", "x.sqlite", "--ic", "42"]).is_ok());
        assert!(Cli::try_parse_from([
            "recall",
            "show",
            "--db",
            "x.sqlite",
            "--message-id",
            "message-42",
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "recall",
            "show",
            "--db",
            "x.sqlite",
            "--reference",
            "ref:ic/42/uuid/message-42",
        ])
        .is_ok());
        assert!(Cli::try_parse_from(["recall", "show", "--db", "x.sqlite"]).is_err());
        assert!(Cli::try_parse_from([
            "recall",
            "show",
            "--db",
            "x.sqlite",
            "--ic",
            "42",
            "--message-id",
            "message-42",
        ])
        .is_err());
    }
}
