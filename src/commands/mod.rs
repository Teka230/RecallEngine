pub mod browse;
pub mod export;
pub mod import;
pub mod search;
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
        Commands::Tools { db } => export::tools::run(db),
        Commands::Serve {
            db,
            assets_dir,
            host,
            port,
            allow_remote,
        } => serve::run(db, assets_dir, host, port, allow_remote),
        Commands::Export { target } => match target {
            ExportTarget::LegacySqlite { db, output } => {
                let out =
                    output.unwrap_or_else(|| db.parent().unwrap().join("exports/legacy.sqlite"));
                export::legacy_sqlite::run_legacy_sqlite(db, out)
            }
            ExportTarget::Markdown { db, ic, out } => {
                let out = out.unwrap_or_else(|| db.parent().unwrap().join("exports/markdown"));
                export::markdown::run_markdown(db, ic, out)
            }
            ExportTarget::Bundle {
                db,
                out,
                profile,
                format,
                force,
            } => {
                let out = out
                    .unwrap_or_else(|| db.parent().unwrap().join(format!("exports/{}", profile)));
                export::bundles::run_bundle(db, out, profile, format, force)
            }
        },
        Commands::Search {
            db,
            query,
            syntax,
            count_mode,
            role,
            ic_min,
            ic_max,
            date_min,
            date_max,
            limit,
            offset,
            json,
        } => search::run_search(
            db, query, syntax, count_mode, role, ic_min, ic_max, date_min, date_max, limit, offset,
            json,
        ),
    }
}
