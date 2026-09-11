# rust-utils: Decouple the HTTP Stack From a Pinned Crypto Provider

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a consumer of the `query` feature choose its `rustls` crypto provider, instead of
having `aws-lc-rs` forced on it by reqwest's default features.

**Architecture:** Three changes in `jluszcz/rust-utils`, then verification across its five
consumers. `tls` keeps meaning aws-lc-rs so every current consumer is untouched; a new `tls-ring`
feature offers ring; reqwest drops its default features so it pins no provider of its own.

**Tech Stack:** Rust 2024, `reqwest` 0.13, `rustls` 0.23, `backon`. CI is
`jluszcz/github-utils/.github/workflows/rust-ci.yml@v2` on `ubuntu-24.04-arm`, target
`aarch64-unknown-linux-musl`, `all-features: true`.

**Spec:** `docs/superpowers/specs/2026-09-11-fund-allocation-lookthrough-design.md` in the
MisterManager repository — the "Dependency: `jluszcz_rust_utils`" section. This plan is the
upstream half of it and lands in a different repository.

**Repository:** `/Users/jacob/Documents/Programs/rust-utils` — **not** MisterManager. Every path in
this plan is relative to that checkout.

## Global Constraints

- **`main` must keep building for repos that haven't opted into a new feature yet.** Consumers track
  this crate as an unpinned git dependency. Copied from `CLAUDE.md`. This is the binding constraint
  on every task here: a consumer that does not edit its `Cargo.toml` must be unaffected.
- **Features are additive and default-off.**
- Never disable a failing test; fix it.
- Never commit directly to the default branch — create a feature branch first.
- Commit with the `jluszcz:commit` skill.
- Documentation describes the code as it is, not how it got there.
- Every commit message ends with:
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LPunW61FrbUCTazxaNj23Y
  ```

### Commands

```bash
cargo test --all-features
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings

# Feature-isolated builds — the ones that actually exercise this change.
cargo tree -e features -i rustls --no-default-features --features query
cargo tree -i aws-lc-sys --no-default-features --features query
```

`aarch64-unknown-linux-musl` is **not** installed locally and nothing in the repo declares it. Run
`rustup target add aarch64-unknown-linux-musl` only if reproducing a CI failure; the tasks below do
not need it, because the property under test is which crates resolve, not whether they cross-compile.

---

## Context an implementer needs

**Why this change exists.** reqwest 0.13's feature graph is:

```
default → default-tls → rustls → __rustls-aws-lc-rs → aws-lc-sys
```

`rustls` here is reqwest's own feature name, and it pins aws-lc-rs. rust-utils currently takes
reqwest with default features on, so **every `query` consumer compiles `aws-lc-sys` whether or not
it wants it**. A consumer that has already chosen ring elsewhere (via `aws-sdk-s3`'s `rustls`
feature, say) ends up with two crypto backends in one binary.

reqwest's `rustls-no-provider` feature gives the same rustls stack without choosing a provider,
leaving the choice to the application.

**Why `tls` is not simply switched to ring.** `lambda::run` calls `tls::install_default_provider()`,
and four consumers reach it through `lambda`. Switching the provider under them would change the
crypto backend of four production Lambdas as a side effect of an unrelated refactor. `tls` keeps
meaning aws-lc-rs; ring arrives as a second, opt-in feature.

**What happens if nobody installs a provider.** `rustls` refuses to build a connection and the first
HTTPS request fails at runtime. Today that cannot happen, because the only two `query` consumers also
take `lambda`, which installs one at cold start. It becomes reachable for the first time with a
`query`-only consumer, which is what makes Task 3's documentation load-bearing rather than decorative.

---

## File Structure

| File | Change |
|---|---|
| `Cargo.toml` | `rustls` becomes provider-less with two feature arms; `reqwest` drops default features. |
| `src/tls.rs` | `install_default_provider` becomes provider-selectable; module gate widens. |
| `src/query.rs` | Doc comment on `http_client` stating the provider precondition. |
| `README.md` | Feature table gains `tls-ring`; TLS and HTTP-client sections restated. |
| `CLAUDE.md` | Architecture list restated for the two TLS features. |

---

## Task 1: `tls-ring`, alongside the existing `tls`

**Files:**
- Modify: `Cargo.toml` (the `[features]` block and the `rustls` dependency line)
- Modify: `src/tls.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: feature `tls-ring`; `tls::install_default_provider()` keeps its exact signature
  `pub fn install_default_provider()` and stays callable under either feature.

- [ ] **Step 1: Write the failing test**

Replace the existing `mod tests` in `src/tls.rs` with this. The second test is new; the first is
the existing one, unchanged, and is repeated here so the block can be pasted whole.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_install_is_idempotent_and_leaves_a_provider() {
        install_default_provider();
        install_default_provider();

        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }

    /// The installed provider is the one the enabled feature names.
    ///
    /// `tls` wins when both are on, which is what keeps a consumer that adds
    /// `tls-ring` to an existing `lambda` build on the backend it already had.
    #[test]
    fn test_installed_provider_matches_the_enabled_feature() {
        install_default_provider();
        let provider = rustls::crypto::CryptoProvider::get_default()
            .expect("a provider is installed");

        #[cfg(feature = "tls")]
        let expected = rustls::crypto::aws_lc_rs::default_provider();
        #[cfg(all(feature = "tls-ring", not(feature = "tls")))]
        let expected = rustls::crypto::ring::default_provider();

        assert_eq!(
            provider.cipher_suites.len(),
            expected.cipher_suites.len(),
            "installed provider is not the one the enabled feature names"
        );
    }
}
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test --no-default-features --features tls-ring tls::`

Expected: FAIL — `error: none of the selected packages contains these features: tls-ring`.

- [ ] **Step 3: Make `rustls` provider-less and add the feature arm**

In `Cargo.toml`, replace the `rustls` dependency line:

```toml
rustls  = { version = "0.23", default-features = false, optional = true }
```

and in `[features]`, replace the `tls` line with these two:

```toml
tls = ["dep:rustls", "rustls/aws_lc_rs"]
tls-ring = ["dep:rustls", "rustls/ring"]
```

Leave `lambda = ["tls", "dep:lambda_runtime", "dep:serde"]` exactly as it is — that is what keeps
the four `lambda` consumers on aws-lc-rs with no edit of their own.

- [ ] **Step 4: Make the installer select on the feature**

In `src/tls.rs`, replace the body of `install_default_provider` and restate its doc comment. The
doc says what the function does now, not that it changed:

```rust
/// Installs the process-wide `rustls` crypto provider.
///
/// Which provider depends on the enabled feature: `tls` installs `aws-lc-rs`,
/// `tls-ring` installs `ring`, and `tls` wins when both are on. `rustls`
/// refuses to build a TLS connection when more than one provider is compiled
/// in and none has been chosen, which surfaces as a runtime failure on the
/// first HTTPS request rather than at build time. Calling this before any TLS
/// work removes that failure mode.
///
/// Safe to call repeatedly and from anywhere: if a provider is already
/// installed, this leaves it alone. [`crate::lambda::run`] calls it for you;
/// binaries that aren't Lambdas — including any that take `query` without
/// `lambda` — call it themselves at the top of `main`.
pub fn install_default_provider() {
    #[cfg(feature = "tls")]
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    #[cfg(all(feature = "tls-ring", not(feature = "tls")))]
    let _ = rustls::crypto::ring::default_provider().install_default();
}
```

In `src/lib.rs`, widen the module gate so `tls-ring` alone reaches it:

```rust
#[cfg(any(feature = "tls", feature = "tls-ring"))]
pub mod tls;
```

- [ ] **Step 5: Run the tests under each feature and make sure they pass**

```bash
cargo test --no-default-features --features tls-ring tls::
cargo test --no-default-features --features tls tls::
cargo test --all-features tls::
```

Expected: PASS in all three.

- [ ] **Step 6: Confirm each arm resolves to the provider it names**

```bash
cargo tree -i aws-lc-sys --no-default-features --features tls-ring
```

Expected: `error: package ID specification 'aws-lc-sys' did not match any packages` — that error
**is** the pass condition. Then:

```bash
cargo tree -i ring --no-default-features --features tls-ring
```

Expected: `ring` present, depended on by `rustls`.

- [ ] **Step 7: Lint, format, commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
git add Cargo.toml Cargo.lock src/tls.rs src/lib.rs
```

Commit with the `jluszcz:commit` skill. Message:
`feat(tls): offer ring as a second crypto provider`

---

## Task 2: reqwest stops pinning a provider

**Files:**
- Modify: `Cargo.toml` (the `reqwest` dependency line)
- Modify: `src/query.rs` (the `http_client` doc comment)

**Interfaces:**
- Consumes: `tls-ring` from Task 1.
- Produces: no signature changes. `query::http_client() -> Result<&'static Client>`,
  `query::send`, `query::http_get` and `query::http_get_json` all keep their exact signatures.

**What this must not break:** `JakeSky-rs` and `mbtalerts` take `["bedrock", "cli", "lambda",
"query"]`. They keep `aws-lc-sys` via `lambda → tls`, and `lambda::run` still installs the provider
at cold start. Neither repo is edited. Task 3 verifies that claim rather than trusting it.

- [ ] **Step 1: Write the failing test**

This property is about crate resolution, not runtime behaviour, so it is a shell assertion rather
than a `#[test]`. Create `scripts/check-features.sh`:

```bash
#!/usr/bin/env bash
# `query` alone must pull no crypto provider: the application chooses one.
set -euo pipefail

fail=0

if cargo tree -i aws-lc-sys --no-default-features --features query >/dev/null 2>&1; then
  echo "FAIL: aws-lc-sys is reachable from --features query"
  fail=1
else
  echo "ok: query pulls no aws-lc-sys"
fi

if cargo tree -i ring --no-default-features --features query >/dev/null 2>&1; then
  echo "FAIL: ring is reachable from --features query"
  fail=1
else
  echo "ok: query pulls no ring"
fi

if cargo tree -i aws-lc-sys --no-default-features --features query,tls >/dev/null 2>&1; then
  echo "ok: query,tls pulls aws-lc-sys"
else
  echo "FAIL: query,tls does not pull aws-lc-sys"
  fail=1
fi

if cargo tree -i ring --no-default-features --features query,tls-ring >/dev/null 2>&1; then
  echo "ok: query,tls-ring pulls ring"
else
  echo "FAIL: query,tls-ring does not pull ring"
  fail=1
fi

exit "$fail"
```

Then `chmod +x scripts/check-features.sh`.

- [ ] **Step 2: Run it to make sure it fails**

Run: `./scripts/check-features.sh`

Expected: FAIL on the first check — `aws-lc-sys is reachable from --features query`, because
reqwest's defaults are still on.

- [ ] **Step 3: Drop reqwest's default features**

In `Cargo.toml`, replace the `reqwest` line. `system-proxy`, `http2` and `charset` are carried over
explicitly because they were in reqwest's `default` and dropping them silently would change
behaviour for every consumer:

```toml
# `default` resolves to `default-tls` -> `rustls` -> `__rustls-aws-lc-rs`, which
# pins aws-lc-rs on every consumer of `query`. `rustls-no-provider` is the same
# TLS stack with the choice left to the application, which is what lets a
# non-Lambda consumer stay on ring. The three features below `rustls-no-provider`
# were in reqwest's own `default` set and are carried over by name.
reqwest = { version = "0.13", default-features = false, features = [
    "gzip", "json", "query", "rustls-no-provider", "http2", "charset", "system-proxy",
], optional = true }
```

- [ ] **Step 4: Run it to make sure it passes**

Run: `./scripts/check-features.sh`

Expected: all four lines `ok:`, exit 0.

- [ ] **Step 5: State the precondition where a caller will read it**

In `src/query.rs`, extend the `http_client` doc comment. Add this paragraph after the existing
description of the client's configuration:

```rust
/// **A `rustls` crypto provider must be installed before the first request.**
/// This crate's reqwest build pins none, so the application chooses: enable
/// `tls` or `tls-ring` and call [`crate::tls::install_default_provider`], which
/// [`crate::lambda::run`] already does for Lambda binaries. Without one, the
/// first HTTPS request fails at runtime rather than the build failing.
```

- [ ] **Step 6: Full suite, lint, format**

```bash
cargo test --all-features
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
./scripts/check-features.sh
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/query.rs scripts/check-features.sh
```

Commit with the `jluszcz:commit` skill. Message:
`fix(query): leave the crypto provider to the application`

---

## Task 3: Verify all five consumers

**Files:** none in this repository. This task edits nothing and exists to prove the Global
Constraint held.

**Interfaces:**
- Consumes: Tasks 1 and 2, committed and pushed to `main` (consumers track the git branch, so
  nothing is verifiable until the change is on `main`).
- Produces: a verified claim that no consumer needs an edit.

**The five consumers**, all under `/Users/jacob/Documents/Programs/`:

| Repository | Features | Expected effect |
|---|---|---|
| `todoer` | `cli` | None — reaches neither `query` nor any TLS feature. |
| `LogStreamGC` | `aws, cli, lambda` | None — no `query`. Keeps aws-lc-rs via `lambda → tls`. |
| `ListOfLists-rs` | `aws, cli, lambda` | None — same. |
| `JakeSky-rs` | `bedrock, cli, lambda, query` | Uses `query`. Keeps aws-lc-rs via `lambda → tls`; `lambda::run` installs it at cold start. |
| `mbtalerts` | `bedrock, cli, lambda, query` | Same as `JakeSky-rs`. |

- [ ] **Step 1: Refresh each consumer's lock against the new `main`**

For each of the five, in its own checkout:

```bash
cargo update -p jluszcz_rust_utils
```

- [ ] **Step 2: Confirm the two `query` consumers still resolve a provider**

For `JakeSky-rs` and `mbtalerts`:

```bash
cargo tree -i aws-lc-sys
```

Expected: present, reached via `rustls` from `jluszcz_rust_utils`'s `tls` feature. If it is
**absent**, the Global Constraint has been violated — `lambda::run` would install nothing and the
first HTTPS request would fail in production. Stop and fix Task 1's feature wiring before going on.

- [ ] **Step 3: Build and test each consumer**

For each of the five:

```bash
cargo build --all-features
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: PASS in all five. Any failure is a Task 1 or Task 2 defect, not a consumer defect — fix it
upstream rather than editing the consumer.

- [ ] **Step 4: Commit each refreshed lockfile, or record that none moved**

`cargo update -p` rewrites `Cargo.lock` only where the resolution actually changed. For each
consumer whose lock moved:

```bash
git -C <consumer> add Cargo.lock
```

Commit with the `jluszcz:commit` skill, on a feature branch in that repository. Message:
`chore(deps): take rust-utils' provider-less reqwest build`

Where a lock did not move, record that in the handoff rather than making an empty commit.

---

## Task 4: Documentation

**Files:**
- Modify: `README.md` — the TLS provider section, the HTTP client section, the feature table
- Modify: `CLAUDE.md` — the architecture list's TLS line

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces: nothing consumed downstream.

- [ ] **Step 1: Restate the TLS section in `README.md`**

Replace the `### TLS provider (...)` section with:

```markdown
### TLS provider (`tls::install_default_provider`) — features `tls`, `tls-ring`

Installs the process-wide `rustls` crypto provider, which otherwise fails on the first HTTPS request
rather than at build time. `tls` installs `aws-lc-rs`; `tls-ring` installs `ring`, which avoids
compiling C. `tls` wins when both are enabled. Idempotent. `lambda::run` calls it (the `lambda`
feature implies `tls`); every other binary calls it at the top of `main`, including one that takes
`query` without `lambda`.
```

- [ ] **Step 2: State the precondition in the HTTP client section**

Append to the `### HTTP client (`query::http_client`) — feature `query`` section:

```markdown
The client pins no `rustls` crypto provider, so a consumer chooses one: enable `tls` or `tls-ring`
and call `tls::install_default_provider` before the first request.
```

- [ ] **Step 3: Update the feature table**

In the `## Features` table, replace the `tls` row with:

```markdown
| `tls` | `rustls` crypto provider installation (`aws-lc-rs`) |
| `tls-ring` | The same, on `ring` — no C compilation |
```

- [ ] **Step 4: Update `CLAUDE.md`**

In the Core Components list, replace the TLS line:

```markdown
- **TLS** (`tls`, features `tls` / `tls-ring`) - `rustls` crypto provider installation, on `aws-lc-rs` or `ring`
```

- [ ] **Step 5: Verify the docs match the code**

```bash
grep -n 'tls-ring' README.md CLAUDE.md Cargo.toml src/tls.rs
```

Expected: `tls-ring` appears in all four. A feature named in `Cargo.toml` and in no doc is the
failure this step catches.

- [ ] **Step 6: Commit**

```bash
git add README.md CLAUDE.md
```

Commit with the `jluszcz:commit` skill. Message:
`docs: state which crypto provider each TLS feature installs`
