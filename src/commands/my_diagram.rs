use serenity::all::{Context, CreateCommand, ResolvedOption, UserId};

use crate::AppError;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use unidb::diesel_schema::vestibule_users;
use unidb::models::VestibuleUserRecord;

#[tracing::instrument(
    skip_all,
    fields(username = %command_user_id.to_string())
)]
pub async fn run(
    ctx: &Context,
    _options: &[ResolvedOption<'_>],
    command_user_id: UserId,
    pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
) -> Result<(String, Option<Vec<u8>>), AppError> {
    let mut conn = pool
        .get()
        .await
        .map_err(|e| AppError::AppError(Box::new(e)))?;

    let user_id_str = command_user_id.get().to_string();

    let record_opt: Option<VestibuleUserRecord> = vestibule_users::table
        .filter(vestibule_users::discord_user_id.eq(&user_id_str))
        .select(VestibuleUserRecord::as_select())
        .first(&mut conn)
        .await
        .optional()
        .map_err(AppError::DatabaseError)?;

    let record = match record_opt {
        Some(r) => r,
        None => {
            return Ok((
                "You haven't posted an introduction or haven't been scored yet!".to_string(),
                None,
            ));
        }
    };

    let diagram_bytes = match record.intro_diagram {
        Some(bytes) => bytes,
        None => {
            return Ok((
                "Your diagram is still being processed or could not be generated.".to_string(),
                None,
            ));
        }
    };

    Ok((
        "Here is your HEXACO personality diagram!".to_string(),
        Some(diagram_bytes),
    ))
}

pub fn register() -> CreateCommand {
    CreateCommand::new("my_diagram")
        .description("Get your HEXACO personality diagram based on your intro")
}
