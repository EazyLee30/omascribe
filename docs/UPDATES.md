# Signed desktop updates

The native release workflow builds four targets, verifies their signed updater
artifacts and version binding, tests corruption/downgrade rejection, then publishes
`latest.json` alongside all installers. No manifest is published for a partial build.
macOS uses `.app.tar.gz`, Windows the NSIS `.exe`, and Linux the `.AppImage`.

`TAURI_SIGNING_PRIVATE_KEY` is a GitHub Actions repository secret. Its matching public
key lives in `src-tauri/tauri.conf.json`. Keep an offline backup of the private key;
losing it breaks the trust chain for existing clients. Never commit or print it.
Key rotation requires a separately planned trust transition. Release `tauri build`
signs with the app version; `requireSignedVersion` rejects relabeled older packages.

Settings → Software updates is opt-in, manual and separate from model downloads.
Checks transmit only a public GET request to the fixed GitHub manifest URL.
Downloads follow GitHub's asset redirects and verify the payload before it is cached
and again before installation. Cancelling drops the request; disabling permission
also cancels it. The checked update is held in memory only for the current process.
Cached bytes alone cannot authorize installation after restart.

User data is outside the application bundle. Finish recording/transcription, speaker
analysis and note generation before installation. No automatic restart occurs.
Debian packages remain package-managed. Version 0.3.0 has no updater and
must be upgraded manually once.

Platform validation reports cover installed inference and signature rejection.
Interactive installer/relaunch behavior must additionally be tested on each OS;
an updater manifest with a newer version is required for a normal client offer.
