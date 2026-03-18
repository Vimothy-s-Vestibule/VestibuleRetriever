use serenity::all::{CreateCommand, ResolvedOption, UserId};

use crate::AppError;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use unidb::diesel_schema::{scores, vestibule_users};
use unidb::models::{ScoreRecord, VestibuleUserRecord};

#[tracing::instrument(
    skip_all,
    fields(username = %command_user_id.to_string())
)]
pub async fn run(
    _options: &[ResolvedOption<'_>],
    command_user_id: UserId,
    pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
) -> Result<(String, Option<Vec<u8>>), AppError> {
    let mut conn = pool
        .get()
        .await
        .map_err(|e| AppError::AppError(Box::new(e)))?;

    let user_id_str = command_user_id.get().to_string();

    let user_record: Option<VestibuleUserRecord> = vestibule_users::table
        .filter(vestibule_users::discord_user_id.eq(&user_id_str))
        .select(VestibuleUserRecord::as_select())
        .first(&mut conn)
        .await
        .optional()
        .map_err(AppError::DatabaseError)?;

    let record = match user_record {
        Some(r) => r,
        None => {
            return Ok(("You are not in the user database.".to_string(), None));
        }
    };

    let score_id = match record.score_id {
        Some(id) => id,
        None => {
            return Ok(("You haven't been scored yet!".to_string(), None));
        }
    };

    let score_record = match scores::table
        .filter(scores::score_id.eq(&score_id))
        .select(ScoreRecord::as_select())
        .first(&mut conn)
        .await
        .optional()
        .map_err(AppError::DatabaseError)?
    {
        Some(r) => r,
        None => {
            return Ok((
                "Your intro diagram has not been generated as your intro has not been processed yet, check back later.".to_string(),
                None,
            ));
        }
    };

    let diagram_bytes = match score_record.intro_diagram {
        Some(bytes) => bytes,
        None => {
            return Ok((
                "Your intro diagram has not been generated as your intro has not been processed yet, check back later.".to_string(),
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
