# Releasing

## Bump the version

Set the same version in `npm/package.json`, `crates/htp-decompress/Cargo.toml` and
`crates/htp-decompress-wasm/Cargo.toml`, run `cargo build` so `Cargo.lock` follows, and commit. `release.yml`
refuses to run unless the tag is `v` plus the version in all three files.

## First release, by hand

npm only lets you attach a trusted publisher to a package that already exists, so the first version is
published from your machine:

    npm login
    mise run build
    cd npm && npm pack
    npm publish htp-decompress-0.1.0.tgz --access public

Publishing the tarball from `npm pack` matches what `release.yml` does.

Then on npmjs.com open the package, go to Settings, Trusted Publisher, and add a GitHub Actions publisher:

- Repository: `four-random-common-words/htp-decompress`
- Workflow filename: `release.yml`
- Environment: leave empty
- Allowed actions: leave "Allow `npm publish`" and "Allow `npm dist-tag`" unchecked. `npm stage publish` is
  always allowed, so CI can stage a version but only you can make it live.

Then push the tag `v0.1.0` (as below). The workflow sees that the version is already on npm, skips the
publish, and creates the GitHub release with the tarball.

## Later releases

Push a tag after bumping the versions:

    git tag v0.1.1
    git push origin v0.1.1

`release.yml` runs two jobs. `build`, with read-only permissions, runs the checks, packs the tarball and
uploads it as a workflow artifact. `publish`, the only job that can request an OIDC token and write
releases, downloads that tarball, stages it on npm with provenance through trusted publishing (no npm token
is stored), and creates a GitHub release with it attached.

The staged version is not live until you approve it with your npm second factor:

    mise exec -- npm stage list htp-decompress
    mise exec -- npm stage download <stage-id>
    mise exec -- npm stage approve <stage-id> --otp <code>

`npm stage download` lets you inspect the tarball first; `npm stage reject <stage-id>` drops it. You can
also approve or reject on npmjs.com. Until you approve, the GitHub release points at a version npm does not
serve yet, so approve right after the run.

If `publish` fails after staging, approve the staged version first, then open the run and use "Re-run failed
jobs": the job takes the same tarball from `build` again, skips staging because the version is now on npm,
and finishes the release. "Re-run all jobs" would build a new tarball, which may not match the staged one
byte for byte.

Trusted publishing needs npm 11.5.1 or newer and `npm stage` a newer one still; the pinned Node 24.18.0
ships npm 11.16.0, which has both.

## Moving the repository to the hashtopolis organization

- Update the trusted publisher on npmjs.com to the new owner and repository name.
- Update the `repository` field in `npm/package.json` and in all `Cargo.toml` files.
- Keep the workflow filename `release.yml`, or update it in the trusted publisher as well.
