# What's new in 1.0

OwnHealth 1.0 is the connected release. It pulls records from US patient portals and Medicare, reads wearables through their APIs and account exports, opens hospital imaging CDs, shares a summary through a SMART Health Link, and keeps each profile's files under its own key. Every 0.x install upgrades in place and keeps its data. This page lists what changed and then [how to upgrade](#upgrading-from-0x). The [changelog](https://github.com/GeiserX/OwnHealth/blob/main/CHANGELOG.md#100-2026-11-26) has every change and the known limits.

## What changed

- **US patient portals and Medicare.** Connect Epic, Oracle Health, athenahealth, eClinicalWorks, MEDITECH, NextGen or VA through SMART on FHIR, with your own registered vendor app. Medicare claims come in through CMS Blue Button. Both sync every day ([connectors](connectors/README.md)).
- **Wearables.** The optional `wearables` compose profile runs Open Wearables for Oura, WHOOP, Polar and Withings. Garmin, Strava and Fitbit account exports, FIT files, and LibreView and Dexcom Clarity CSVs import too ([connectors](connectors/README.md)).
- **Imaging.** The optional `imaging` compose profile adds Orthanc. Import a hospital CD as one study and open it next to its report.
- **Sharing.** Send a doctor a scoped summary through a SMART Health Link, with an optional PIN, an expiry, an access log and revoke. A scannable emergency card gives first responders minimal offline text.
- **Emergency access.** A relative you name can ask for access. It opens only after a delay, once your calendar feed has shown the request, and you can cancel it.
- **Per-profile keys.** Each profile's documents, genome files and connector tokens are encrypted under its own key, so deleting a profile makes its files unreadable.
- **Who sees what.** A member without the grant no longer sees another profile's sensitive details, and a FHIR security label marks a document and its facts sensitive.
- **Languages.** German, French, Catalan, Italian and Portuguese are beta languages, in the app and on every printed sheet.

## Upgrading from 0.x

Passwords, passkeys, access tokens and OAuth connections keep working. Twenty-three migrations apply at start. Migrations are forward-only: to go back, restore the backup you take in step 1. Coming from 0.3.0 or earlier? Read the "To 0.4.0" and "To 0.3.0" notes in [Upgrades](https://github.com/GeiserX/OwnHealth/blob/main/deploy/README.md#upgrades) first, because rule pack 0.2.0 moves every `state_hash` once.

1. Take a backup while the compose file still names your old version: `./deploy/restic/backup.sh`.
2. Set `admin` and `app` to `drumsergio/ownhealth:1.0.0` and `ocr` to `drumsergio/ownhealth:1.0.0-ocr`, and take the new compose file.
3. Before `docker compose up -d`, do the three steps below: the imaging folder (homelab only), the keyring folder and the secrets key.
4. Start it with `docker compose pull && docker compose up -d`. Then take a backup, and [encrypt the older files](#encrypt-the-older-files) when you choose.

### The imaging folder (homelab)

The new `compose.homelab.yaml` mounts `secrets/imaging` into the app, with the `imaging` profile on or off. Create the folder before the first `docker compose up -d` with that file: `mkdir -m 755 secrets/imaging`. If Docker started first, it made the folder as root: `sudo chown "$(id -u):$(id -g)" secrets/imaging && chmod 755 secrets/imaging`. To turn imaging on, follow the comment above the `orthanc` service in `compose.homelab.yaml`: it writes `secrets/imaging/orthanc_password` with mode 644.

### The keyring folder

Both Compose services mount `./keyring/<project>`. For the default project, create `keyring/ownhealth` before you start Compose, as the [homelab setup](https://github.com/GeiserX/OwnHealth/blob/main/deploy/README.md#homelab-setup) shows. If you use `-p` or `COMPOSE_PROJECT_NAME`, use that project name in place of `ownhealth`. If Docker created the folder, give UID and GID 10001 ownership and set mode 0700.

If your old Compose file mounted `./keyring` directly, stop the old project before switching Compose files. Move its current `keyring/keyring.json` into `keyring/<project>/keyring.json` before starting the updated project. Move the file to the project that owns it. Take a backup right after the upgrade: it writes `keyring.json`. Backups taken before the upgrade do not hold the profile keys: keep `keyring.json`, or they restore without their files. Tell your host backup tools to skip `keyring/`.

### The secrets key

OwnHealth now encrypts each profile's documents and genome files, so every command except `migrate` and `healthcheck` refuses to start without `OWNHEALTH_SECRETS_KEY` and prints one line that says how to make one. Keep the key you have: it already encrypts your inbox secrets, and a new one would lock them.

- Homelab: the new `compose.homelab.yaml` gives `secrets/secrets_key` to `admin` and `ocr` as well as the app (backups and restores open the encrypted files). The file exists since 0.3.0, so on an upgrade from 0.3.0 or later take the new compose file and nothing else changes. On a direct upgrade from 0.2.0 or earlier, run the steps below first, next to `compose.homelab.yaml`: they keep a key file you already have, move the key from `.env` when there is no file, and make a new key only when neither has one.
- Quickstart (`compose.yaml`): the key now comes from the file `secrets/secrets_key`, never from `.env`. These steps keep a key file you already have, move the value from `.env` when there is no file, and make a new key only when neither has one. Then delete the `OWNHEALTH_SECRETS_KEY=` line from `.env`:

```sh
mkdir -p -m 700 secrets
if test -s secrets/secrets_key; then
  echo "secrets/secrets_key is there: keeping it"
elif v=$(sed -n -E 's/^[[:space:]]*(export[[:space:]]+)?OWNHEALTH_SECRETS_KEY[[:space:]]*=[[:space:]]*//p' .env | tail -n 1 |
    sed -E "s/[[:space:]]+#.*\$//; s/[[:space:]]+\$//; s/^\"(.*)\"\$/\\1/; s/^'(.*)'\$/\\1/") && test -n "$v"; then
  printf '%s' "$v" > secrets/secrets_key   # the value Compose reads, without quotes or a comment
else
  openssl rand -base64 32 | tr -d '\n' > secrets/secrets_key   # neither had a key
fi
chmod 644 secrets/secrets_key
```

If OwnHealth then stops with "does not open ... of the ... profile keys", the key is not the one the install was made with. Nothing was changed: put the original key back in `secrets/secrets_key` (from the old `.env`, or from the copy you keep with the backups) and start again. A new key never opens files already stored.

### Encrypt the older files

Files stored before the upgrade stay where they are and open as before; files stored after it are encrypted. Take a backup after the upgrade and keep the key next to the restic password. Then encrypt the older files and connection tokens too, with a job that runs only when you start it:

```sh
docker compose run --rm -T admin ownhealth seal-legacy --dry-run   # what it would do; changes nothing
docker compose run --rm -T admin ownhealth seal-legacy
```

The job stores each older file under the key of every profile or pending item that reads it. It keeps the unencrypted copy if an ownerless reader still needs it or no record names it. The job also encrypts each older connection refresh token under its profile's key. It can run while OwnHealth runs, and a backup that runs at the same time keeps every file it needs. If you stop the job, run it again to resume.

If legacy passwords or tokens exist, `OWNHEALTH_SECRETS_KEY` must open at least one before the job writes anything. The command prints a JSON report and exits 1 if it could not encrypt a file or token. The report counts copies that landed before a later copy failed. The `errors` list identifies failed files by their SHA-256 and lists document IDs when available. If a file fails its SHA-256 check, restore it from a backup or retract its document, then run the job again.

Inbox records from before 1.0 did not say which profile they were for. The job gives each one the profile of its document, of the documents its released mail made, or of its inbox when the inbox is a profile's own, and then encrypts its file. Some files stay unencrypted, and the report counts them under `files.kept` and lists them under `files.kept_files`:

- `ownerless`: reference data, or a file of a household inbox from before 1.0 that no document or release ties to a profile (a mail with only a body, a scan that made no document).
- `unnamed`: a file no record names any more. Nothing reads it.

A run after a complete one reports the same kept files again. Backups taken before the job still hold the unencrypted files until retention removes them.

### What else to check

Rule pack 0.2.0 stays, so every profile's `state_hash` stays as it was on 0.4.0. `db/app-role.sql` now revokes CONNECT on the `ownhealth` database from PUBLIC: a role you added yourself, such as a monitoring login, needs `grant connect on database ownhealth to <role>`. The `imaging` and `wearables` profiles and sharing stay off until you turn them on. With the homelab stack, update the `src` checkout first (`git -C src pull`), then update the deployed Caddyfile from `src/deploy/Caddyfile` and keep your local access restrictions: the 0.4.0 file has no share listener on port 8090, so sharing links and public wearable callbacks fail without it. Then run `docker compose up -d --force-recreate caddy` to load it.
