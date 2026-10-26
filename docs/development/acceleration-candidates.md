# Optional GPU acceptance candidates

Base installers contain no CUDA/HIP/Vulkan SDKs or optional GPU runtime payloads. Hardware detection is a recommendation; a signed catalog, verified member hashes and an actual local model test remain prerequisites to activation. Metal on Apple Silicon uses the OS runtime separately.

## Reuse the complete build already in progress

Windows started full run `40367160891` at `164371e285a499e9261afa5239d1d563a1aadb57`, version `acceleration-ci-40367160891`. Do not start another SDK matrix just to sign it. At this checkpoint its CUDA jobs are still running, so **no complete candidate/catalog/signature is claimed yet**. The catalog job is scheduled only after all dependencies succeed; its absence while builds run is expected.

1. Let that run finish. All Windows/Linux × CUDA/Vulkan builds, Windows core and the original catalog must succeed, with five non-expired artifacts. Two Vulkan successes alone are insufficient.
2. Remove `build-acceleration-components` after its intended run has started; removing a label does not cancel that run. Merge the latest shared #1 into Windows normally, keeping its native linking/driver fixes. Application CI and Windows-core checks must pass for this exact head before signing.
3. The shared maintainer pre-creates the immutable lightweight tag `acceleration-ci-40367160891` at the verified original build source `164371e285a499e9261afa5239d1d563a1aadb57`, using the existing maintainer connection with workflow permission. This publishes no component files. GitHub's [release permission rules](https://docs.github.com/en/rest/releases/releases#create-a-release) prevent `GITHUB_TOKEN` from creating a tag that targets workflow changes; the signer therefore uses `--verify-tag` and never creates or retargets one. Windows does not need to expand its OAuth permissions. A missing/wrong tag fails preflight before downloads or key use.
4. Add the one-shot `sign-acceleration-run-40367160891` label to the same-repository PR. The separate **Sign existing acceleration candidate** workflow runs on that explicit label addition. A later source push does not re-sign. Remove the label after the intended run; no SDK compiler is invoked by this workflow.
5. The signing job verifies original workflow success, artifact completeness and every native job's original checkout. The run's `pull_requests` head/base values can follow later PR updates, so they are not treated as trigger snapshots. Each retained checkout log must identify one fixed commit before the Python setup step; that commit must have the exact run-head tree, and a synthetic PR merge must contain the run head and its ancestor base. All four jobs must succeed; missing/ambiguous logs or differing trees refuse reuse. Checkout revisions, tree/base and log SHA-256 are retained in provenance. A native-input fingerprint separately covers the component build and runtime-inventory scripts, runtime lock, runner/matrix configuration and exact SDK/build/package steps. Delivery orchestration/acceptance-test changes may reuse the compiled binary; a compiler/SDK/package input change cannot. All members are rehashed, and the reconstructed catalog must equal the original payload byte for byte before signing.
6. The existing app release key signs those exact bytes. The app verifier checks the embedded public key and signed version. The release starts as an unpublished draft, uploads only the four separate components, signed index and provenance note, then checks remote asset names, sizes and GitHub SHA-256 digests. Only a complete matching candidate is exposed. An existing immutable tag is never overwritten. A failed draft remains for inspection; an operator must remove only that unpublished failed draft/tag before retrying the same original version. This does not require another SDK build.

The source of the compiled SDK artifact and the newer publishing source are recorded separately. Unsigned provenance is an audit aid; app trust remains the signed payload plus archive/member hashes. No candidate label updates `acceleration-v1`, the app's default component directory, latest installer or application update manifest. [GitHub release asset digests](https://docs.github.com/en/rest/releases/assets) are checked after upload, in addition to local verification.

The repository's default branch currently lacks the component workflow (confirmed by its contents endpoint returning 404). Manual dispatch is therefore not used as a workaround to merge main early. The same-repository explicit label provides a limited candidate path. [GitHub workflow event rules](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_dispatch) explain default-branch dispatch availability. Fork PRs cannot use the signing path.

## Test through the real settings/download path

Use a developer app that includes this shared checkpoint and Windows fixes, with a separate test data directory. For the above immutable candidate, launch it with:

```powershell
$env:OMASCRIBE_DATA_DIR = 'C:\OmaScribe-validation\gpu-candidate'
$env:OMASCRIBE_ACCELERATION_CANDIDATE_VERSION = 'acceleration-ci-40367160891'
# Launch the developer application normally in this process environment.
```

The selector also accepts `acceleration-candidate-*` tags for future candidates. It is a **version selector**, not an arbitrary URL override. The origin remains this repository's release URL; signature, runtime revision, exact selected version, archive and member checks are unchanged. Candidate caches are separate from `acceleration-index.json`, so an old stable catalog cannot masquerade as a successful candidate fetch. Removing the variable restores the stable catalog selection. Isolate test data to keep existing installed components and activation records out of the acceptance trial.

Exercise hardware detection → fetch signed candidate catalog → download → local Test & enable → restart → CPU fallback/disable → removal. Save source/runtime/model/driver hashes, actual offload and fact/negation evidence, latency/resources, failed work and loaded module paths. A clean runtime PATH and absence of CUDA SDK variables must establish that only the component's distributable runtime and system driver are used. No unsigned catalog, manually seeded installed marker, machine-wide Vulkan override or full CUDA SDK can substitute for the intended installed-user path. Windows owns the GTX 1650 driver and native acceptance; unavailable AMD/Linux hardware paths remain experimental.

## Later release promotion

For a newly built full matrix, the existing workflow can also sign an immutable candidate: on a same-repository PR, both `build-acceleration-components` and `sign-acceleration-candidate` are required. Single Windows-Vulkan builds cannot create catalogs. All checks/builds use the exact source, and quick Windows checks use a separate concurrency group so they do not queue behind a long SDK build.

After actual device acceptance, manual dispatch with `publish=true` creates an immutable signed release against a pre-created exact source tag; `promote=false` is the default. Only an explicit separate `promote=true` updates `acceleration-v1`, after full package, signature and remote upload gates. Its initial pointer tag must also be pre-created by the maintainer; existing pointer tags are never moved. Promotion is never enabled by a PR label. Driver/reset/OOM/rollback and the broader platform matrix still require their own evidence. No stable pointer has been published by this checkpoint.
