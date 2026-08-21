//! The upload, and the only place `aws_config`, `aws_sdk_s3` and `tokio` are
//! named.
//!
//! The runtime is built for the duration of one upload and dropped. Nothing
//! else in the crate is async, and this boundary is not a staging post for
//! making it so: SQLite is blocking, `Connection` is `!Sync`, and the TUI's
//! `event::poll(TICK)` is a deadline loop by design.

use anyhow::{Context, Result, anyhow};
use aws_config::BehaviorVersion;
use aws_sdk_s3::primitives::ByteStream;
use aws_smithy_types::error::display::DisplayErrorContext;
use std::path::Path;

/// One `PutObject`. No multipart: the object is megabytes against a 5 GB
/// single-request limit, and multipart would need the `ListBucket` and abort
/// permissions the IAM policy deliberately withholds.
///
/// No server-side-encryption header either. The bucket's default encryption
/// applies to every object written to it, and naming an algorithm here is one
/// more thing that can disagree with the bucket.
/// The region comes from the profile, not from this application's config: the
/// profile is where `aws configure set region` already puts it, and a second
/// copy could only ever disagree with the first.
pub fn upload(profile: &str, bucket: &str, key: &str, file: &Path) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting the runtime for the upload")?;

    runtime.block_on(async {
        let config = aws_config::defaults(BehaviorVersion::latest())
            .profile_name(profile)
            .load()
            .await;

        // Said by name here. An S3 request with no region fails as
        // `PermanentRedirect`, or as a dispatch failure, neither of which
        // tells the reader which file is missing a line.
        if config.region().is_none() {
            return Err(anyhow!(
                "no region for profile {profile}: run \
                 `aws configure set region <region> --profile {profile}`, \
                 or set AWS_REGION"
            ));
        }

        let client = aws_sdk_s3::Client::new(&config);
        let body = ByteStream::from_path(file)
            .await
            .with_context(|| format!("reading {}", file.display()))?;

        client
            .put_object()
            .bucket(bucket)
            .key(key)
            .body(body)
            .content_type("application/vnd.sqlite3")
            .send()
            .await
            // `DisplayErrorContext` rather than the error's own `Display`:
            // an `SdkError` prints as "service error" on its own, and the
            // part worth reading -- AccessDenied, NoSuchBucket, an expired
            // key -- is in the source chain it wraps.
            .map_err(|e| anyhow!("{}", DisplayErrorContext(&e)))
            .with_context(|| format!("uploading to s3://{bucket}/{key}"))?;

        Ok(())
    })
}
