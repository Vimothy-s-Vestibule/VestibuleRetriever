use serenity::all::{CreateCommand, ResolvedOption, UserId};

use crate::AppError;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use unidb::diesel_schema::{scores, vestibule_users};
use unidb::models::{ScoreRecord, VestibuleUserRecord};

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

    let Some(record) = vestibule_users::table
        .filter(vestibule_users::discord_user_id.eq(&user_id_str))
        .select(VestibuleUserRecord::as_select())
        .first(&mut conn)
        .await
        .optional()
        .map_err(AppError::DatabaseError)?
    else {
        return Ok((
            "You haven't posted an introduction or haven't been added to the database yet!"
                .to_string(),
            None,
        ));
    };

    let Some(score_id) = record.score_id else {
        return Ok((
            "Your intro hasn't been processed yet. Please check back later!".to_string(),
            None,
        ));
    };

    let Some(score_record) = scores::table
        .filter(scores::score_id.eq(&score_id))
        .select(ScoreRecord::as_select())
        .first(&mut conn)
        .await
        .optional()
        .map_err(AppError::DatabaseError)?
    else {
        return Ok((
            "Something went wrong: Your score record could not be found.".to_string(),
            None,
        ));
    };

    let Some(diagram_bytes) = score_record.intro_diagram else {
        return Ok((
            "Your personality diagram is still being generated. Please check back later!"
                .to_string(),
            None,
        ));
    };

    Ok((
        "Here is your HEXACO personality diagram!".to_string(),
        Some(diagram_bytes),
    ))
}

pub fn register() -> CreateCommand {
    CreateCommand::new("my_diagram")
        .description("Get your HEXACO personality diagram based on your introduction.")
}
