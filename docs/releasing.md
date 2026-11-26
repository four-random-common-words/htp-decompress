# Releasing

## Bump the version

Set the same version in `npm/package.json`, `crates/htp-decompress/Cargo.toml` and
`crates/htp-decompress-wasm/Cargo.toml`, run `cargo build` so `Cargo.lock` follows, and commit. `release.yml`
refuses to run unless the tag is `v` plus the version in all three files.

## First release, by hand

npm only lets you attach a trusted publisher to a package that already exists, so the first version is
published from your machine:

    npm login
    cd npm && npm run build && npm pack
    npm publish htp-decompress-0.1.0.tgz --access public

Publishing the tarball from `npm pack` matches what `release.yml` does.

Then on npmjs.com open the package, go to Settings, Trusted Publisher, and add a GitHub Actions publisher:

- Repository: `four-random-common-words/htp-decompress`
- Workflow filename: `release.yml`
- Environment: leave empty

Then push the tag `v0.1.0` (as below). The workflow sees that the version is already on npm, skips the
publish, and creates the GitHub release with the tarball.

## Later releases

Push a tag after bumping the versions:

    git tag v0.1.1
    git push origin v0.1.1

`release.yml` runs two jobs. `build`, with read-only permissions, runs the checks, packs the tarball and
uploads it as a workflow artifact. `publish`, the only job that can request an OIDC token and write
releases, downloads that tarball, publishes it with provenance through trusted publishing (no npm token is
stored), and creates a GitHub release with it attached.

If `publish` fails after the publish, open the run and use "Re-run failed jobs": the job takes the same
tarball from `build` again, skips the publish because the version is already on npm, and finishes the
release. "Re-run all jobs" would build a new tarball, which may not match the published one byte for byte.

Trusted publishing needs npm 11.5.1 or newer; the pinned Node 24.18.0 ships npm 11.16.0.

## Moving the repository to the hashtopolis organization

- Update the trusted publisher on npmjs.com to the new owner and repository name.
- Update the `repository` field in `npm/package.json` and in all `Cargo.toml` files.
- Keep the workflow filename `release.yml`, or update it in the trusted publisher as well.
