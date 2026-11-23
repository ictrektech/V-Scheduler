use super::*;

use serde::Serialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProfileRegistration {
    name: String,
}

#[derive(Serialize, sqlx::FromRow)]
struct ComputeProfile {
    name: String,
    builtin: bool,
}

fn valid_profile_name(name: &str) -> bool {
    name.len() <= 64
        && name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && name.split('-').all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
        && !matches!(name, "auto" | "thor" | "thor-spark" | "l4t")
}

async fn access(app: &App, headers: &HeaderMap) -> Api<bool> {
    match auth(app, headers).await {
        Ok(principal) if principal.role == "submitter" => Ok(false),
        Ok(principal) => {
            require(&principal, "admin")?;
            Ok(true)
        }
        Err(Error(StatusCode::FORBIDDEN, _)) => crate::identity::verified_account(headers)
            .await
            .map_err(|status| Error(status, "FORBIDDEN")),
        Err(error) => Err(error),
    }
}

pub(super) async fn list_profiles(
    State(app): State<App>,
    headers: HeaderMap,
) -> Api<Json<Value>> {
    let can_manage = access(&app, &headers).await?;
    let items = sqlx::query_as::<_, ComputeProfile>(
        "SELECT name, builtin FROM vrs_compute_profiles ORDER BY builtin DESC, name",
    )
    .fetch_all(&app.db)
    .await
    .map_err(unavailable)?;
    Ok(ok(json!({ "items": items, "can_manage": can_manage })))
}

pub(super) async fn register_profile(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<ProfileRegistration>,
) -> Api<Json<Value>> {
    require(&auth(&app, &headers).await?, "admin")?;
    let name = input.name.trim();
    if !valid_profile_name(name) {
        return Err(invalid());
    }
    let profile = sqlx::query_as::<_, ComputeProfile>(
        "INSERT INTO vrs_compute_profiles (name) VALUES ($1)
         ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name RETURNING name, builtin",
    )
    .bind(name)
    .fetch_one(&app.db)
    .await
    .map_err(unavailable)?;
    Ok(ok(json!(profile)))
}

#[derive(Deserialize)]
pub(super) struct Filter {
    profile: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Registration {
    image_ref: String,
    profiles: Vec<String>,
}

#[derive(Serialize, sqlx::FromRow)]
struct Image {
    image_ref: String,
    profiles: Vec<String>,
    tested_profiles: Vec<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn valid_image_ref(reference: &str) -> bool {
    if reference.len() > 512 || reference.contains("://") || !reference.is_ascii() {
        return false;
    }
    let Some((registry, name)) = reference.split_once('/') else {
        return false;
    };
    if !(registry.contains('.') || registry.contains(':') || registry == "localhost")
        || !registry.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b".:-[]".contains(&b)
        })
        || reqwest::Url::parse(&format!("https://{registry}")).is_err()
    {
        return false;
    }
    let (name, has_digest) = match name.split_once('@') {
        Some((name, digest)) => {
            let Some(hash) = digest.strip_prefix("sha256:") else {
                return false;
            };
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
                return false;
            }
            (name, true)
        }
        None => (name, false),
    };
    let repository = match name.rsplit_once(':') {
        Some((repository, tag)) => {
            if tag.is_empty() || tag.len() > 128
                || !tag.as_bytes()[0].is_ascii_alphanumeric() && !tag.starts_with('_')
                || !tag.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            {
                return false;
            }
            repository
        }
        None if has_digest => name,
        None => return false,
    };
    repository.split('/').all(|part| {
        let mut bytes = part.bytes();
        bytes.next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && part.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
    })
}

pub(super) async fn list_images(
    State(app): State<App>,
    headers: HeaderMap,
    Query(filter): Query<Filter>,
) -> Api<Json<Value>> {
    let can_manage = access(&app, &headers).await?;
    let compatible = match filter.profile.as_deref() {
        None => Vec::new(),
        Some("dgx-spark") => vec!["dgx-spark", "jetson-thor"],
        Some(profile) if valid_profile_name(profile) => vec![profile],
        Some(_) => return Err(invalid()),
    };
    let items = sqlx::query_as::<_, Image>(
        "SELECT image_ref, profiles, tested_profiles, created_at, updated_at FROM vrs_inference_images
         WHERE cardinality($1::text[]) = 0 OR profiles && $1::text[] ORDER BY image_ref",
    )
    .bind(compatible)
    .fetch_all(&app.db)
    .await
    .map_err(unavailable)?;
    Ok(ok(json!({ "items": items, "can_manage": can_manage })))
}

pub(super) async fn register_image(
    State(app): State<App>,
    headers: HeaderMap,
    Json(mut input): Json<Registration>,
) -> Api<Json<Value>> {
    require(&auth(&app, &headers).await?, "admin")?;
    let reference = input.image_ref.trim();
    if !valid_image_ref(reference)
        || input.profiles.is_empty()
        || input.profiles.len() > 64
    {
        return Err(invalid());
    }
    input.profiles.sort_unstable();
    input.profiles.dedup();
    let registered: i64 =
        sqlx::query_scalar("SELECT count(*) FROM vrs_compute_profiles WHERE name = ANY($1)")
            .bind(&input.profiles)
            .fetch_one(&app.db)
            .await
            .map_err(unavailable)?;
    if registered != input.profiles.len() as i64 {
        return Err(Error(StatusCode::UNPROCESSABLE_ENTITY, "COMPUTE_PROFILE_NOT_REGISTERED"));
    }
    let image = sqlx::query_as::<_, Image>(
        "INSERT INTO vrs_inference_images (image_ref, profiles) VALUES ($1, $2)
         ON CONFLICT (image_ref) DO UPDATE SET profiles = EXCLUDED.profiles,
           updated_at = CASE WHEN vrs_inference_images.profiles <> EXCLUDED.profiles
                             THEN now() ELSE vrs_inference_images.updated_at END
         RETURNING image_ref, profiles, tested_profiles, created_at, updated_at",
    )
    .bind(reference)
    .bind(input.profiles)
    .fetch_one(&app.db)
    .await
    .map_err(unavailable)?;
    Ok(ok(json!(image)))
}

#[cfg(test)]
mod tests {
    use super::valid_image_ref;

    #[test]
    fn public_image_addresses_accept_registry_tags_and_digests_only() {
        assert!(valid_image_ref("registry.example.com/team/vllm:0.1"));
        assert!(valid_image_ref(&format!(
            "localhost:5000/team/vllm@sha256:{}", "a".repeat(64)
        )));
        for reference in [
            "vllm:latest",
            "registry.example.com/team/vllm",
            "https://registry.example.com/team/vllm:0.1",
            "user:password@registry.example.com/team/vllm:0.1",
            "registry.example.com/team/vllm:0.1?token=secret",
            "registry.example.com/team/vllm@sha256:bad",
            "registry.example.com/../vllm:0.1",
        ] {
            assert!(!valid_image_ref(reference), "{reference}");
        }
    }
}
