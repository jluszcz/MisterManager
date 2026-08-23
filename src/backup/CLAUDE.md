# backup — the schedule, the snapshot, and the upload

`aws_config`, `aws_sdk_s3` and `tokio` are named only in `s3.rs`. The identity this code runs as,
and the bucket it writes to, are declared in `mistermanager.tf` at the repository root — two of the
invariants below span that file as well as this directory, and say so where they do.

What the schedule is *for* is a database the owner cannot lose; what bounds it is that the key is
long-lived and unattended. Both halves are below.

## Invariants

- **An absent config file means backups are off; an unparseable one is an error; a key nothing
  reads is ignored.** The first is the same rule an unset `setting` key follows, and is what makes a
  clean checkout and an unconfigured machine both do nothing. The second is what stops a misspelled
  key from leaving `bucket` unset and reading as "off", because a backup that silently stops running
  is the one failure nothing downstream ever notices — and what does that work is `bucket` having
  **no default**, so the dangerous typo is a missing required field rather than a stray one. That is
  what makes the third safe: an unknown key is skipped, so a file written for another build still
  configures every key this one does understand.
- **`interval_days` is clamped before it reaches `TimeDelta::days`, the same rule as every
  user-editable setting that reaches a divisor.** `backup::interval` caps it at `MAX_INTERVAL_DAYS`
  (3653, ten years) because the value is read straight out of a hand-edited config file and
  `DateTime`'s addition panics rather than erroring once the sum leaves chrono's calendar — a nonsense
  setting must not take the run down, the same reasoning behind `div_ceil`'s `.max(1)` callers and the
  recurring transaction horizon's `1..=120`-month clamp.
- **The backup state file is advisory where a `setting` key is binding.** An unreadable
  `~/.local/state/mistermanager/backup.toml` warns and is treated as "never backed up", rather than
  refusing the way a dangling `setting` key does. The asymmetry is in the consequence: a dangling
  setting key moves real money to the wrong place, while a corrupt state file costs one redundant
  upload and is correct again as soon as it is rewritten.
- **The backup identity may only `PutObject`.** The key in the `mistermanager` profile is
  long-lived and unattended, so the policy is what bounds it: it cannot read a backup, delete one,
  or list the prefix, and restores are done by the owner under their own identity.
- **The bucket is this repository's own, and its name is composed rather than configured.**
  `mistermanager-<account id>-<region>-an`, built in `mistermanager.tf` from
  `aws_caller_identity` and `var.aws_region`. A name that had to be *chosen* would say where the
  owner's finances are backed up — the same kind of fact as the workbook path — and would have to
  reach Terraform out of band to stay unsaid; one derived from the profile is safe to commit and
  needs no such step. Owning the bucket is what makes the
  lifecycle rule declarable at all: `aws_s3_bucket_lifecycle_configuration` is a whole-bucket
  resource, so two repositories declaring one would revert each other on every apply. The rule
  expires an object at 365 days.
- **The scheduled check is the default database's, and `--db` opts out of it.** It runs after every
  arm but `backup` itself, and the state file it stamps records *when* an upload last happened
  rather than *what* was uploaded — so a scratch database backed up on the schedule would take the
  real one's turn as well as leaving an object nothing distinguishes from a real backup. An
  explicit `mm backup` is exempt, because being pointed somewhere is what it was asked for.
- **The due check reads the real clock, not `--today`.** `--today` simulates a financial date, and
  whether a file reached S3 is a fact about wall time — `mm --today 2027-01-01` must not fire an
  upload. `run_if_due` therefore takes `now` as `Utc::now()` from the CLI rather than the `today`
  the rest of the application is driven by.
- **The key prefix is a constant, not a setting, because one end of it is an IAM policy.**
  `mistermanager` appears in `backup::PREFIX` and in `mistermanager.tf`'s policy resource path, and
  nothing ties them together — but the policy is what the IAM user is scoped to, and only an AWS
  apply can change it, so a config knob could only ever be turned into `AccessDenied`. Moving the
  prefix means editing both, in that order. `Backup` carries no `prefix` field, so a config file
  asking for another prefix is a line that does nothing.
