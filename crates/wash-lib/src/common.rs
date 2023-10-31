use std::str::FromStr;

use crate::id::ModuleId;

const CLAIMS_CALL_ALIAS: &str = "call_alias";
pub(crate) const CLAIMS_NAME: &str = "name";
pub(crate) const CLAIMS_SUBJECT: &str = "sub";

/// Converts error from Send + Sync error to standard anyhow error
pub(crate) fn boxed_err_to_anyhow(e: Box<dyn ::std::error::Error + Send + Sync>) -> anyhow::Error {
    anyhow::anyhow!(e.to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum FindIdError {
    /// No matches were found
    #[error("No actor found with the search term")]
    NoMatches,
    /// Multiple matches were found. The vector contains the list of actors that matched
    #[error("Multiple actors found with the search term: {0:?}")]
    MultipleMatches(Vec<String>),
    #[error(transparent)]
    Error(#[from] anyhow::Error),
}

/// Given a string, attempts to resolve an actor ID. Returning the actor ID and an optional friendly name
///
/// If the string is a valid actor ID, it will be returned unchanged. Resolution works by checking
/// if the actor_name or call_alias fields from the actor's claims contains the given string. If
/// more than one matches, then an error will be returned indicating the options to choose from
pub async fn find_actor_id(
    value: &str,
    ctl_client: &wasmcloud_control_interface::Client,
) -> Result<(ModuleId, Option<String>), FindIdError> {
    if let Ok(id) = ModuleId::from_str(value) {
        return Ok((id, None));
    }

    // Case insensitive searching here to make things nicer
    let value = value.to_lowercase();
    // If it wasn't an ID, get the claims
    let claims = ctl_client
        .get_claims()
        .await
        .map_err(|e| FindIdError::Error(anyhow::anyhow!("Unable to get claims: {}", e)))?;
    let all_matches = claims
        .iter()
        .filter_map(|v| {
            let id = v
                .get(CLAIMS_SUBJECT)
                .map(|s| s.as_str())
                .unwrap_or_default();
            // If it isn't a module, just skip
            let id = match ModuleId::from_str(id) {
                Ok(id) => id,
                Err(_) => return None,
            };
            (v.get(CLAIMS_CALL_ALIAS)
                .map(|s| s.to_lowercase())
                .unwrap_or_default()
                .contains(&value)
                || v.get(CLAIMS_NAME)
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_default()
                    .contains(&value))
            .then(|| (id, v.get(CLAIMS_NAME).map(|s| s.to_string())))
        })
        .collect::<Vec<_>>();
    if all_matches.is_empty() {
        Err(FindIdError::NoMatches)
    } else if all_matches.len() > 1 {
        Err(FindIdError::MultipleMatches(
            all_matches
                .into_iter()
                .map(|(id, friendly_name)| {
                    if let Some(name) = friendly_name {
                        format!("{} ({})", id, name)
                    } else {
                        id.into_string()
                    }
                })
                .collect(),
        ))
    } else {
        // SAFETY: We know we have exactly one match at this point
        Ok(all_matches.into_iter().next().unwrap())
    }
}
