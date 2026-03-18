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

    if !crate::all_elements_unique(&existing_user_ids) {
        tracing::error!("There are duplicate users in the database",);
    }

    tracing::info!(
        "Found {} existing users in database",
        existing_user_ids.len()
    );

    let mut members = Vec::new();
    let mut last_member_id = None;
    loop {
        let chunk = guild_id
            .members(&ctx.http, Some(1000), last_member_id)
            .await
            .map_err(AppError::SerenityError)?;

        if chunk.is_empty() {
            break;
        }

        last_member_id = Some(chunk.last().unwrap().user.id);
        let is_last_chunk = chunk.len() < 1000;
        members.extend(chunk);

        if is_last_chunk {
            break;
        }
    }

    tracing::info!("Fetched {} members from the guild", members.len());

    // 1. Identify who is missing an intro
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
            tracing::info!("Introductions for all users found.");
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
            tracing::info!("Reached the beginning of the introductions channel, stopping search.");
            break;
        }

        for message in &messages {
            let author_id = message.author.id;

            // If this message belongs to someone we are looking for
            if missing_users.remove(&author_id) {
                let discord_msg = DiscordMessage {
                    user_id: author_id.get().to_string(),
                    content: message.content.clone(),
                    message_id: message.id.get().to_string(),
                    sent_at: *message.timestamp,
                    added_at: chrono::Utc::now(),
                    score_id: None,
                };

                tracing::info!(
                    "Found and inserting introduction message for user: {}",
                    author_id
                );

                if let Err(e) = crate::insert_introduction_message(&mut conn, &discord_msg).await {
                    tracing::error!("Failed to store message for {}: {}", author_id, e);
                    db_errors += 1;
                } else {
                    scraped_count += 1;
                }
            }
        }

        before = messages.last().map(|message| message.id);

        // If we got fewer than 100 messages, we're at the very beginning of the channel
        if messages.len() < 100 {
            tracing::info!("Reached the beginning of the introductions channel, stopping search.");
            break;
        }
    }

    let skipped_count = members.len() - initial_missing_count;

    tracing::info!(
        "Finished scrape_intros execution. Scraped: {}, Skipped (already in DB with an intro): {}, No intro found: {}, DB Errors: {}",
        scraped_count,
        skipped_count,
        missing_users.len(),
        db_errors
    );

    Ok((
        format!(
            "Scraped {} new messages.\nSkipped (already in db): {}\nNo intro found (may include early admins wihtout an intro): {}",
            scraped_count,
            skipped_count,
            missing_users.len(),
        ),
        None,
    ))
}

pub fn register() -> CreateCommand {
    CreateCommand::new("scrape_intros").description(
        "Scrapes introduction messages from all users that posted an intro (requires SCRAPER_ROLE_ID)",
    )
}
