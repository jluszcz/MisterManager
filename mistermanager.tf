terraform {
  # 1.10 is the floor for use_lockfile below.
  required_version = ">= 1.10"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.37"
    }
  }

  backend "s3" {
    bucket = "jluszcz-tf-state"
    key    = "mistermanager"
    region = "us-east-2"

    # S3-native locking; no DynamoDB table required.
    use_lockfile = true
  }
}

variable "aws_region" {
  type    = string
  default = "us-east-2"
}

provider "aws" {
  region = var.aws_region

  default_tags {
    tags = {
      ManagedBy = "terraform"
      Repo      = "MisterManager"
    }
  }
}

data "aws_caller_identity" "current" {}

/**************************************
* Backup Bucket
**************************************/

# The name is composed, not chosen, and that is what lets it be written down in a
# public repository. A name someone picked would say where the owner's finances
# are backed up -- the same kind of fact as the workbook path MM_WORKBOOK carries,
# and it would have to reach Terraform out of band to stay unsaid. Built from the
# caller's own account and region, it is derivable by whoever already holds the
# profile and says nothing to anyone who does not.
resource "aws_s3_bucket" "backup" {
  bucket           = format("mistermanager-%s-%s-an", data.aws_caller_identity.current.account_id, var.aws_region)
  bucket_namespace = "account-regional"
}

resource "aws_s3_bucket_public_access_block" "backup" {
  bucket = aws_s3_bucket.backup.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# The AWS-managed aws/s3 key, stated rather than left to the account default,
# because the IAM policy below leans on it: that key's own policy grants
# same-account principals use of it via kms:ViaService, so the backup identity
# needs no kms:GenerateDataKey grant. A bucket quietly switched to a
# customer-managed key would fail every upload with an error about a key rather
# than about a policy.
resource "aws_s3_bucket_server_side_encryption_configuration" "backup" {
  bucket = aws_s3_bucket.backup.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "aws:kms"
    }
    bucket_key_enabled = true

    # Declared to match what S3 already enforces. Leaving it out makes the
    # provider plan it away on every run, which would silently permit
    # customer-provided-key uploads that the bucket currently rejects.
    blocked_encryption_types = ["SSE-C"]
  }
}

# Owned outright, which is the point of a bucket of this application's own: a
# lifecycle configuration is a whole-bucket resource, so this rule can only be
# written where nothing else is declaring one.
resource "aws_s3_bucket_lifecycle_configuration" "backup" {
  bucket = aws_s3_bucket.backup.id

  rule {
    id     = "expire"
    status = "Enabled"

    # Every object in the bucket. It exists for the backups and holds nothing
    # else.
    filter {}

    expiration {
      days = 365
    }
  }

  rule {
    id     = "expire-noncurrent"
    status = "Enabled"

    noncurrent_version_expiration {
      noncurrent_days = 30
    }
  }

  rule {
    id     = "abort-mpu"
    status = "Enabled"

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}

/**************************************
* Backup IAM User
**************************************/

resource "aws_iam_user" "mistermanager" {
  name = "mistermanager"
}

resource "aws_iam_access_key" "mistermanager" {
  user = aws_iam_user.mistermanager.name
}

data "aws_iam_policy_document" "mistermanager" {
  # PutObject and nothing else. This key is long-lived and sits unattended on
  # a laptop, so what bounds the damage is the policy rather than the key's
  # lifetime: a stolen key can add objects under the prefix, but cannot read a
  # backup, delete one, or list what is there. Restores are done by the owner
  # under their own SSO identity.
  #
  # No KMS statement is needed: the bucket encrypts under the AWS-managed aws/s3
  # key above, whose key policy already grants same-account principals use of it
  # via kms:ViaService. A move to a customer-managed key would need
  # kms:GenerateDataKey added here.
  statement {
    actions   = ["s3:PutObject"]
    resources = ["${aws_s3_bucket.backup.arn}/mistermanager/*"]
  }
}

resource "aws_iam_user_policy" "mistermanager" {
  name   = "mistermanager"
  user   = aws_iam_user.mistermanager.name
  policy = data.aws_iam_policy_document.mistermanager.json
}

# What goes in config.toml's `bucket`. Derivable from the profile, but not worth
# assembling by hand.
output "backup_bucket" {
  value = aws_s3_bucket.backup.bucket
}

# The secret is otherwise reachable only by reading raw state.
output "mistermanager_access_key_id" {
  value = aws_iam_access_key.mistermanager.id
}

output "mistermanager_access_key_secret" {
  value     = aws_iam_access_key.mistermanager.secret
  sensitive = true
}
