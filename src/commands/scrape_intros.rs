use serenity::all::{
    ChannelId, Context, CreateCommand, GetMessages, ResolvedOption, RoleId, UserId,
};

use crate::AppError;
use std::collections::HashSet;
use unidb::models::DiscordMessage;

async fn has_role(
    ctx: &Context,
    guild_id: serenity::model::id::GuildId,
    user_id: UserId,
    role_id: RoleId,
) -> Result<bool, AppError> {
    let member = guild_id
        .member(&ctx.http, user_id)
        .await
        .map_err(AppError::SerenityError)?;
    Ok(member.roles.contains(&role_id))
}

#[tracing::instrument(
    skip_all,
    fields(username = %command_user_id.to_string())
)]
pub async fn run(
    ctx: &Context,
    _options: &[ResolvedOption<'_>],
    guild_id: serenity::model::id::GuildId,
    command_user_id: UserId,
    pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
    role_id: RoleId,
    channel_id: ChannelId,
) -> Result<(String, Option<Vec<u8>>), AppError> {
    if !has_role(ctx, guild_id, command_user_id, role_id).await? {
        return Err(AppError::PermissionDenied(
            "You do not have the required role.".into(),
        ));
    }

    tracing::info!("Starting scrape_intros command execution...");

    let existing_user_ids = crate::get_existing_user_ids(pool).await?;
    tracing::info!(
        "Found {} existing users in database",
        existing_user_ids.len()
    );

    let members = guild_id
        .members(&ctx.http, Some(1000), None)
        .await
        .map_err(AppError::SerenityError)?;

    tracing::info!("Fetched {} members from the guild", members.len());

    // 1. Identify exactly who is missing an intro
    let mut missing_users: HashSet<UserId> = HashSet::new();
    let mut user_names = std::collections::HashMap::new();

    for member in &members {
        let user_id = member.user.id;
        if !existing_user_ids.contains(&user_id.get().to_string()) {
            missing_users.insert(user_id);
            user_names.insert(user_id, member.user.name.clone());
        }
    }

    let initial_missing_count = missing_users.len();
    tracing::info!(
        "Found {} users missing intros. Starting channel scan...",
        initial_missing_count
    );

    let mut scraped_count = 0;
    let mut db_errors = 0;
    let mut before = None;

    let mut conn = pool
        .get()
        .await
        .map_err(|e| AppError::AppError(Box::new(e)))?;

    // 2. Scan the channel backwards, crossing off users as we find their messages
    loop {
        if missing_users.is_empty() {
            tracing::info!("All missing users found! Stopping channel scan early.");
            break;
        }

        let mut request = GetMessages::new().limit(100);
        if let Some(message_id) = before {
            request = request.before(message_id);
        }

        let messages = channel_id
            .messages(&ctx.http, request)
            .await
            .map_err(AppError::SerenityError)?;

        if messages.is_empty() {
            tracing::info!("Reached the beginning of the channel.");
            break;
        }

        for message in &messages {
            let author_id = message.author.id;

            // If this message belongs to someone we are looking for...
            if missing_users.remove(&author_id) {
                let username = user_names.get(&author_id).cloned().unwrap_or_default();

                let discord_msg = DiscordMessage {
                    user_id: author_id.get().to_string(),
                    content: message.content.clone(),
                    message_id: message.id.get().to_string(),
                    created_at: *message.timestamp,
                };

                tracing::info!(
                    "Found and inserting introduction message for user: {}",
                    username
                );

                if let Err(e) = crate::insert_introduction_message(&mut conn, &discord_msg).await {
                    tracing::error!("Failed to store message for {}: {}", username, e);
                    db_errors += 1;
                } else {
                    scraped_count += 1;
                }
            }
        }

        before = messages.last().map(|message| message.id);

        // If we got fewer than 100 messages, we're at the very beginning of the channel
        if messages.len() < 100 {
            break;
        }
    }

    // 3. Anyone left in `missing_users` means we scanned the whole channel and found nothing.
    let mut failed_users: Vec<String> = missing_users
        .into_iter()
        .filter_map(|id| user_names.get(&id))
        .map(|name| format!("{}: No messages found in channel", name))
        .collect();

    failed_users.sort();

    let skipped_count = members.len() - initial_missing_count;

    tracing::info!(
        "Finished scrape_intros execution. Scraped: {}, Skipped (already in DB): {}, Not Found: {}, DB Errors: {}",
        scraped_count,
        skipped_count,
        failed_users.len(),
        db_errors
    );

    let failed_summary = if failed_users.is_empty() {
        "None".to_string()
    } else {
        let max_display = 10;
        let display_users: Vec<_> = failed_users.iter().take(max_display).collect();
        let mut s = format!("{:#?}", display_users);
        if failed_users.len() > max_display {
            s.push_str(&format!(
                "\n...and {} more",
                failed_users.len() - max_display
            ));
        }
        s
    };

    Ok((
        format!(
            "Scraped {} new messages.\nSkipped (already in db): {}\nFailed ({} total): {}",
            scraped_count,
            skipped_count,
            failed_users.len(),
            failed_summary
        ),
        None,
    ))
}

pub fn register() -> CreateCommand {
    CreateCommand::new("scrape_intros").description(
        "Scrapes introduction messages from all users in the intro channel (requires SCRAPER_ROLE_ID)",
    )
}
