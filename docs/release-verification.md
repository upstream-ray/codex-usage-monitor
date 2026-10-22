# Release verification

Release artifacts should be tied to a reviewed source commit before they are approved for company accounts. Releases built with this revision's tag-triggered workflow include SHA256 checksums and GitHub build-provenance attestations. There is no Authenticode code-signing step. An attestation establishes where an artifact was built; it is not a security audit or a Windows publisher signature.

## Build controls

The Windows workflows use:

- A specific Rust toolchain version and full commit pins for third-party actions.
- `cargo fmt --all -- --check`, `cargo test --locked`, and focused installer-boundary tests before the release build.
- `cargo build --release --locked`, using the checked-in `Cargo.lock`.
- A managed-installation smoke test on the disposable Windows runner, using `-Managed -NoLaunch` and policy-check helper modes without launching provider polling.
- A release tag that must match the Cargo package version and point to a commit on the repository's default branch.
- SHA256 sidecars for the executable, installer, uninstall helper, and build metadata. `SHA256SUMS` covers these payloads and all their sidecars. The manifest cannot hash itself; its release attestation covers it.
- A GitHub-hosted Windows runner. The release workflow obtains a short-lived OIDC identity for the attestation; no signing key is stored in the repository.

`build-info.json` records the source commit, repository, workflow run, compiler version, and build command. It is useful context, but the cryptographically verified attestation is the release identity evidence.

## Verify a release before executing it

Use a current GitHub CLI that supports `gh attestation verify`. Obtain the expected repository, release tag, and full source commit through your review process. For a fork, use the fork's repository, not the upstream repository.

```powershell
$releaseRepository = 'upstream-ray/codex-usage-monitor'
$releaseTag = '<reviewed release tag>'
$reviewedCommit = '<reviewed 40-character source commit>'

gh release download $releaseTag --repo $releaseRepository --dir .\verified-release
Set-Location .\verified-release

foreach ($file in @('codex-usage.exe', 'install.ps1', 'uninstall.ps1', 'SHA256SUMS')) {
    gh attestation verify $file --repo $releaseRepository --signer-workflow "$releaseRepository/.github/workflows/release.yml" --source-digest $reviewedCommit --source-ref "refs/tags/$releaseTag" --deny-self-hosted-runners
    if ($LASTEXITCODE -ne 0) { throw "Attestation verification failed: $file" }
}
```

Require every verification to succeed. Older releases created before this workflow change do not acquire attestations retroactively. Do not label their checksums as signed provenance.

Verify the downloaded payloads against the now-authenticated manifest:

```powershell
foreach ($line in Get-Content -LiteralPath .\SHA256SUMS) {
    if ($line -notmatch '^([0-9a-fA-F]{64})  ([A-Za-z0-9._-]+)$') {
        throw 'Invalid checksum manifest entry.'
    }
    $expectedHash = $Matches[1]
    $fileName = $Matches[2]
    $actualHash = (Get-FileHash -LiteralPath $fileName -Algorithm SHA256).Hash
    if ($actualHash -ne $expectedHash) { throw "Checksum mismatch: $fileName" }
}
```

After approval, use the [managed installation command](installation.md#managed-installation) with the approved executable hash. If your organization requires Authenticode signatures, it must sign its approved build through its own signing process or wait for a separately configured publisher-signing service. This workflow does not claim to satisfy that requirement.

## Branch and pull-request artifacts

The `Windows build and tests` workflow runs on branch pushes, pull requests, and manual dispatch. It uploads a complete portable package named `codex-usage-windows-x64-<source commit>`. It does not publish a GitHub release or generate a release attestation. Its token has read-only repository permissions.

Review the exact workflow run and source commit before using a preview artifact. Check the hashes in the artifact, and retain the reviewed run ID and commit alongside any company-approved executable hash. A preview build is not an attested release even when its package version matches the upstream release version.

## Maintainer notes

The release workflow needs `contents: write`, `id-token: write`, and `attestations: write` on its release job. Public repositories can generate GitHub artifact attestations with current GitHub plans; private repositories require a supporting GitHub Enterprise Cloud plan. The workflow fails rather than publishing if attestation generation fails.

Keep Actions revisions pinned to full commit SHAs. Review changes before updating them and the Rust version. SHA256 verification cannot protect against a compromised release author who controls both the binary and checksum; consumers should constrain the attestation's repository, source commit, and signer workflow as shown above. Repository protections and source review remain necessary.

Official references:

- [GitHub artifact attestations](https://docs.github.com/en/actions/concepts/security/artifact-attestations)
- [GitHub's attestation action](https://github.com/actions/attest)
- [GitHub CLI attestation verification options](https://cli.github.com/manual/gh_attestation_verify)
- [GitHub Actions security guidance](https://docs.github.com/en/actions/security-for-github-actions/security-guides/security-hardening-for-github-actions)
