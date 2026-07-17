use std::net::IpAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::domain::reference::ContextScope;
use crate::search::{CountMode, SearchRole, SearchSyntax};

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
    Tools {
        #[arg(long)]
        db: PathBuf,
    },
    Serve {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        assets_dir: Option<PathBuf>,
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        #[arg(long, default_value_t = 8788)]
        port: u16,
        /// Allow binding outside loopback. The API has no authentication or TLS.
        #[arg(long)]
        allow_remote: bool,
    },
    Export {
        #[command(subcommand)]
        target: ExportTarget,
    },
    Search {
        #[arg(long)]
        db: PathBuf,
        query: String,
        #[arg(long, value_enum, default_value = "simple")]
        syntax: SearchSyntax,
        #[arg(long, value_enum, default_value = "none")]
        count_mode: CountMode,
        #[arg(long, value_enum)]
        role: Option<SearchRole>,
        #[arg(long)]
        ic_min: Option<i64>,
        #[arg(long)]
        ic_max: Option<i64>,
        #[arg(long)]
        date_min: Option<f64>,
        #[arg(long)]
        date_max: Option<f64>,
        #[arg(long, default_value = "10")]
        limit: u32,
        #[arg(long, default_value = "0")]
        offset: u32,
        #[arg(long)]
        json: bool,
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
        output: Option<PathBuf>,
    },
    Markdown {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        ic: i64,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Bundle {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value = "notebooklm")]
        profile: String,
        #[arg(long, default_value = "dir")]
        format: String,
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
