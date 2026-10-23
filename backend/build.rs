fn main() {
    // sqlx::migrate! embeds these files; a newly added migration must invalidate
    // an existing incremental build as well as a clean release build.
    println!("cargo:rerun-if-changed=migrations");
}
