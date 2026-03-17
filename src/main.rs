use std::env;

use diesel_async::{
    pooled_connection::deadpool::Pool, pooled_connection::AsyncDieselConnectionManager,
    AsyncPgConnection,
};
use vestibule_retriever::{commands, AppError};

#[tokio::main]
#[allow(clippy::result_large_err)]
async fn main() -> Result<(), AppError> {
    let filter = tracing_subscriber::filter::Targets::new()
        .with_target("vestibule_retriever", Level::INFO)
        .with_target("serenity", LevelFilter::OFF);

    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .event_format(
                    tracing_subscriber::fmt::format()
                        .with_file(true)
                        .with_line_number(true),
                )
                .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE),
        )
        .with(filter);

    tracing::subscriber::set_global_default(subscriber).map_err(AppError::TracingError)?;

    dotenvy::dotenv().map_err(AppError::DotenvyError)?;

    // Configure the client with your Discord bot token in the environment.
    let token = env::var("DISCORD_TOKEN").map_err(|e| AppError::AppError(Box::new(e)))?;
    // Set gateway intents, which decides what events the bot will be notified about
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILD_MEMBERS;

    let db_url = env::var("DATABASE_URL").map_err(|e| AppError::AppError(Box::new(e)))?;
    let config = AsyncDieselConnectionManager::<AsyncPgConnection>::new(db_url.clone());
    let pool = Pool::builder(config)
        .max_size(10)
        .build()
        .map_err(|e| AppError::AppError(Box::new(e)))?;

    let scraper_role_id = env::var("SCRAPER_ROLE_ID")
        .map_err(|_| AppError::MissingEnvVar("SCRAPER_ROLE_ID".into()))?
        .parse::<u64>()
        .map_err(|_| AppError::InvalidEnvVar("SCRAPER_ROLE_ID must be a valid u64".into()))?;

    let intro_channel_id = env::var("DISCORD_INTRO_CHANNEL_ID")
        .map_err(|_| AppError::MissingEnvVar("DISCORD_INTRO_CHANNEL_ID".into()))?
        .parse::<u64>()
        .map_err(|_| {
            AppError::InvalidEnvVar("DISCORD_INTRO_CHANNEL_ID must be a valid u64".into())
        })?;

    let handler = Handler {
        pool,
        scraper_role_id: serenity::all::RoleId::new(scraper_role_id),
        intro_channel_id: serenity::all::ChannelId::new(intro_channel_id),
    };

    let mut client = Client::builder(&token, intents)
        .event_handler(handler)
        .await
        .map_err(|e| AppError::AppError(Box::new(e)))?;

    // Perms error on normal systems
    //TODO Modify this path in docker health check
    tokio::fs::write("/tmp/ready", "1")
        .await
        .map_err(|e| AppError::AppError(Box::new(e)))?;

    if let Err(why) = client.start().await {
        tracing::error!("Client error: {}", why);
    }

    Ok(())
}

use serenity::async_trait;
use serenity::builder::{CreateAttachment, EditInteractionResponse};
use serenity::model::application::Interaction;
use serenity::model::gateway::Ready;
use serenity::model::id::GuildId;
use serenity::prelude::*;
use tracing::level_filters::LevelFilter;
use tracing::Level;
use tracing_subscriber::layer::SubscriberExt;

struct Handler {
    pool: Pool<AsyncPgConnection>,
    scraper_role_id: serenity::all::RoleId,
    intro_channel_id: serenity::all::ChannelId,
}

#[async_trait]
impl EventHandler for Handler {
    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Interaction::Command(command) = interaction else {
            return;
        };

        tracing::info!(
            "Received command: {} from {} ({})",
            command.data.name,
            command.user.name,
            command.user.id,
        );

        // Discord requires a response within 3 seconds.
        // Defer immediately so long-running commands (like scrape_intros) don't timeout.
        if let Err(why) = command.defer(&ctx.http).await {
            tracing::warn!("Cannot defer slash command: {}", why);
            return;
        }

        let options = command.data.options();
        // This should never happen
        let guild_id = command.guild_id.unwrap();

        let command_user_id = command.user.id;
        let result: Result<(String, Option<Vec<u8>>), AppError> = match command.data.name.as_str() {
            "scrape_intros" => {
                commands::scrape_intros::run(
                    &ctx,
                    &options,
                    guild_id,
                    command_user_id,
                    &self.pool,
                    self.scraper_role_id,
                    self.intro_channel_id,
                )
                .await
            }
            "my_diagram" => commands::my_diagram::run(&options, command_user_id, &self.pool).await,

            _ => Ok(("not implemented".to_string(), None)),
        };

        let (content, attachment_bytes) = match result {
            Ok((c, bytes)) => (c, bytes),
            Err(e) => {
                tracing::error!("Command '{}' failed: {}", command.data.name, e);
                (
                    "An error occurred. Please try again later.".to_string(),
                    None,
                )
            }
        };

        let mut edit_response = EditInteractionResponse::new().content(content);

        if let Some(bytes) = attachment_bytes {
            let attachment = CreateAttachment::bytes(bytes, "diagram.png");
            edit_response = edit_response.new_attachment(attachment);
        }

        if let Err(why) = command.edit_response(&ctx.http, edit_response).await {
            tracing::warn!("Cannot edit deferred response: {}", why);
        }
    }

    async fn ready(&self, ctx: Context, ready: Ready) {
        let guild_id = GuildId::new(
            env::var("GUILD_ID")
                .expect("GUILD_ID env var has to be present")
                .parse()
                .expect("GUILD_ID env var must be an integer"),
        );

        let _commands = guild_id
            .set_commands(
                &ctx.http,
                vec![
                    commands::scrape_intros::register(),
                    commands::my_diagram::register(),
                ],
            )
            .await
            .map_err(AppError::SerenityError)
            .inspect_err(|e| tracing::error!("Failed to register slash commands: {e}"));

        tracing::info!(
            "Online & ready with username {} in guild {} with commands {:#?}",
            ready.user.name,
            guild_id,
            _commands
                .unwrap()
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<&String>>()
        );
    }
}
