use std::path::PathBuf;

use crate::domain::reference::{ContextScope, MessageReference, ReferencedMessage};
use crate::error::{RecallError, Result};
use crate::read_model::ReadRepository;

const MAX_CONTEXT_MESSAGES: usize = 50;

pub enum MessageTarget {
    Ic(i64),
    MessageId(String),
    Reference(String),
}

pub struct ShowOptions {
    pub db_path: PathBuf,
    pub target: MessageTarget,
    pub before: Option<usize>,
    pub after: Option<usize>,
    pub scope: ContextScope,
    pub as_json: bool,
}

pub fn run(options: ShowOptions) -> Result<()> {
    let before = options.before.unwrap_or(0);
    let after = options.after.unwrap_or(0);
    if before > MAX_CONTEXT_MESSAGES || after > MAX_CONTEXT_MESSAGES {
        return Err(RecallError::msg(format!(
            "before et after doivent être inférieurs ou égaux à {MAX_CONTEXT_MESSAGES}"
        )));
    }

    let repository = ReadRepository::open_read_only(&options.db_path)?;
    let message = resolve_target(&repository, options.target)?;
    if before == 0 && after == 0 {
        if options.as_json {
            println!("{}", serde_json::to_string_pretty(&message)?);
        } else {
            print_message(&message);
        }
    } else {
        let context = repository
            .ic_context_window(message.ic, before, after, options.scope)?
            .ok_or_else(|| RecallError::msg(format!("IC {} not found", message.ic)))?;
        if options.as_json {
            println!("{}", serde_json::to_string_pretty(&context)?);
        } else {
            for (index, message) in context.messages.iter().enumerate() {
                if index > 0 {
                    println!("\n---");
                }
                print_message(message);
            }
        }
    }
    Ok(())
}

fn resolve_target(repository: &ReadRepository, target: MessageTarget) -> Result<ReferencedMessage> {
    match target {
        MessageTarget::Ic(ic) => {
            if ic <= 0 {
                return Err(RecallError::msg("IC must be a positive integer"));
            }
            repository
                .resolve_ic_reference(ic)?
                .ok_or_else(|| RecallError::msg(format!("IC {ic} not found")))
        }
        MessageTarget::MessageId(message_id) => repository
            .resolve_message_id(&message_id)?
            .ok_or_else(|| RecallError::msg(format!("Message {message_id} not found"))),
        MessageTarget::Reference(reference) => {
            let reference = reference.parse::<MessageReference>()?;
            repository.resolve_reference(&reference)?.ok_or_else(|| {
                RecallError::msg(format!("Message {} not found", reference.message_id))
            })
        }
    }
}

fn print_message(message: &ReferencedMessage) {
    println!("{} {}", message.reference, message.role);
    println!(
        "Conversation : {} ({})",
        message.conversation_title, message.conversation_id
    );
    println!("Message      : {}", message.id);
    if let Some(timestamp) = &message.timestamp {
        println!("Date         : {timestamp}");
    }
    println!();
    println!("{}", message.content);
}
