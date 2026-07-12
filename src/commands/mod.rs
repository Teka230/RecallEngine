pub mod browse;
pub mod export;
pub mod import;
pub mod serve;
pub mod show;
pub mod stats;
pub mod verify;

use crate::cli::{Cli, Commands, ExportTarget, ImportTarget};
use crate::error::Result;

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Import { target } => match target {
            ImportTarget::Chatgpt {
                source,
                db,
                assets,
                assets_dir,
                strict,
                seed_legacy_ic,
            } => import::run_chatgpt_import(source, db, assets, assets_dir, strict, seed_legacy_ic),
        },
        Commands::Verify { db } => verify::run(db),
        Commands::Stats { db, json } => stats::run(db, json),
        Commands::Show {
            db,
            ic,
            message_id,
            reference,
            before,
            after,
            scope,
            json,
        } => {
            let target = if let Some(ic) = ic {
                show::MessageTarget::Ic(ic)
            } else if let Some(message_id) = message_id {
                show::MessageTarget::MessageId(message_id)
            } else if let Some(reference) = reference {
                show::MessageTarget::Reference(reference)
            } else {
                return Err(crate::error::RecallError::msg(
                    "one of --ic, --message-id or --reference is required",
                ));
            };
            show::run(show::ShowOptions {
                db_path: db,
                target,
                before,
                after,
                scope,
                as_json: json,
            })
        }
        Commands::Browse {
            db,
            ic,
            conversation,
        } => browse::run(db, ic, conversation),
        Commands::Serve {
            db,
            assets_dir,
            host,
            port,
        } => serve::run(db, assets_dir, host, port),
        Commands::Export { target } => match target {
            ExportTarget::LegacySqlite { db, output } => export::run_legacy_sqlite(db, output),
        },
    }
}
