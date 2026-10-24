# Configuration gallery

The website publishes reviewed files from `profiles/*.json`. There is no anonymous upload endpoint. Only the maintainers may mark an entry `official`; a community entry must use `origin: "community"` even after review. Review confirms schema and provenance, not performance quality.

1. Export a configuration from the current OmaScribe app.
2. Open the website's Configuration gallery, select that JSON file and inspect it locally. The browser does not upload it.
3. Complete public author/device fields and download the generated submission JSON. Check those fields for personal information.
4. Add it under `community/submissions/<id>.json` in a GitHub pull request, or attach it to an issue yourself. The site only opens GitHub; it never submits on your behalf.
5. Maintainers review the configuration, check model/backend support and any benchmark evidence, then move the entry to `profiles/`. Publishing happens after merge and website deployment.

No recordings, transcripts, meeting notes, memory, custom download addresses, local paths, shell commands or network permissions are accepted. Configurations are limited to 64 KiB. App links are limited to 16 KiB and open a preview; applying settings and downloading missing models require separate user actions.

`profile-schema.mjs` runs in both the browser and build. Run `cd landing && npm test` before submitting. Unknown settings fail validation rather than silently changing behavior. Compatible files use app profile schema 1; new backend fields need an app version that supports them. The released 0.2.0 installers predate the new configuration gallery/deep-link integration.

Measured results belong in `docs/benchmarks/*.json` with hardware, runtime/model identity, method, raw runs and limitations. A configuration without matching evidence is labelled unmeasured. Never infer tokens/s from memory size or chip branding.
