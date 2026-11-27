# Upgrading OwnHealth

Every OwnHealth release upgrades in place and keeps your data. This page has the steps for every upgrade, then a section for each release that needs more, newest first. [CHANGELOG.md](CHANGELOG.md) lists every change.

## Every upgrade

Take a fresh backup first, while `compose.yaml` still names the old version: `backup.sh` runs in the `admin` image, and a newer image would write the backup with its own rules before the migrations ran.

```sh
./deploy/restic/backup.sh         # a fresh backup before every upgrade
```

The quickstart `compose.yaml` has no restic repository, so `backup.sh` fails there. Write the backup into a folder of your own instead. Keep it with a copy of `secrets/secrets_key`, or of the `OWNHEALTH_SECRETS_KEY` line in `.env` on an install from before 1.0 that has no key file yet:

```sh
mkdir -p -m 700 backups && sudo chown 10001:10001 backups
docker compose run --rm -T -v "$PWD/backups:/backups" admin ownhealth backup "/backups/$(date +%F-%H%M%S)"
```

Like every backup, it leaves the profile keys out. They stay in `keyring/<project>/keyring.json`, and [Backups](deploy/README.md#backups) says how to keep that file. Never copy the database volume instead: it holds the profile keys, so a copy can reopen a profile you delete later.

Then change the image tag of the `admin` and `app` services in `compose.yaml` to the new version, and the `ocr` service's tag to `<version>-ocr`, and start it:

```sh
docker compose pull && docker compose up -d
```

Migrations are forward-only and run at start, in the `admin` service, before the app comes up. To go back to an older version, restore the backup you took before the upgrade.

## Upgrading to 1.0

OwnHealth 1.0 is the connected release. It pulls records from US patient portals and Medicare, and reads wearables through their APIs and account exports. It also opens hospital imaging CDs, shares a summary through a SMART Health Link, and keeps each profile's files under its own key. Every 0.x install upgrades in place and keeps its data. The [changelog](CHANGELOG.md#100-2026-11-26) has every change and the known limits.

### What changed

- **US patient portals and Medicare.** Connect Epic, Oracle Health, athenahealth, eClinicalWorks, MEDITECH, NextGen or VA through SMART on FHIR, with your own registered vendor app. Medicare claims come in through CMS Blue Button. Both sync every day ([connectors](docs/connectors/README.md)).
- **Wearables.** The optional `wearables` compose profile runs Open Wearables for Oura, WHOOP, Polar and Withings. Garmin, Strava and Fitbit account exports, FIT files, and LibreView and Dexcom Clarity CSVs import too ([connectors](docs/connectors/README.md)).
- **Imaging.** The optional `imaging` compose profile adds Orthanc. Import a hospital CD as one study and open it next to its report.
- **Sharing.** Send a doctor a scoped summary through a SMART Health Link, with an optional PIN, an expiry, an access log and revoke. A scannable emergency card gives first responders minimal offline text.
- **Emergency access.** A relative you name can ask for access. It opens only after a delay, once your calendar feed has shown the request, and you can cancel it.
- **Per-profile keys.** Each profile's documents, genome files and connector tokens are encrypted under its own key, so deleting a profile makes its files unreadable.
- **Who sees what.** A member without the grant no longer sees another profile's sensitive details, and a FHIR security label marks a document and its facts sensitive.
- **Languages.** German, French, Catalan, Italian and Portuguese are beta languages, in the app and on every printed sheet.

### Steps

Passwords, passkeys, access tokens and OAuth connections keep working. Twenty-three migrations apply at start. Migrations are forward-only: to go back, restore the backup you take in step 1. Coming from 0.3.0 or earlier? Read [Upgrading to 0.4.0](#upgrading-to-040) and [Upgrading to 0.3.0](#upgrading-to-030) first, because rule pack 0.2.0 moves every `state_hash` once.

1. Take a backup as in [Every upgrade](#every-upgrade), while the compose file still names your old version.
2. Set `admin` and `app` to `drumsergio/ownhealth:1.0.0` and `ocr` to `drumsergio/ownhealth:1.0.0-ocr`, and take the new compose file.
3. Before `docker compose up -d`, do the three steps below: the imaging folder (homelab only), the keyring folder and the secrets key.
4. Start it with `docker compose pull && docker compose up -d`. Then take a backup, and [encrypt the older files](#encrypt-the-older-files) when you choose.

#### The imaging folder (homelab)

The new `compose.homelab.yaml` mounts `secrets/imaging` into the app, with the `imaging` profile on or off. Create the folder before the first `docker compose up -d` with that file: `mkdir -m 755 secrets/imaging`. If Docker started first, it made the folder as root: `sudo chown "$(id -u):$(id -g)" secrets/imaging && chmod 755 secrets/imaging`. To turn imaging on, follow the comment above the `orthanc` service in `compose.homelab.yaml`: it writes `secrets/imaging/orthanc_password` with mode 644.

#### The keyring folder

Both Compose services mount `./keyring/<project>`. For the default project, create `keyring/ownhealth` before you start Compose, as the [homelab setup](deploy/README.md#homelab-setup) shows. If you use `-p` or `COMPOSE_PROJECT_NAME`, use that project name in place of `ownhealth`. If Docker created the folder, give UID and GID 10001 ownership and set mode 0700.

If your old Compose file mounted `./keyring` directly, stop the old project before switching Compose files. Move its current `keyring/keyring.json` into `keyring/<project>/keyring.json` before starting the updated project. To go back to the old version later, move it back to `keyring/keyring.json` before you start the old Compose file. Move the file to the project that owns it. Take a backup right after the upgrade: it writes `keyring.json`. Backups taken before the upgrade do not hold the profile keys: keep `keyring.json`, or they restore without their files. Tell your host backup tools to skip `keyring/`.

#### The secrets key

OwnHealth now encrypts each profile's documents and genome files, so every command except `migrate` and `healthcheck` refuses to start without `OWNHEALTH_SECRETS_KEY` and prints one line that says how to make one. Keep the key you have: it already encrypts your inbox secrets, and a new one would lock them.

- Homelab: the new `compose.homelab.yaml` gives `secrets/secrets_key` to `admin` and `ocr` as well as the app (backups and restores open the encrypted files). The file exists since 0.3.0, so on an upgrade from 0.3.0 or later take the new compose file and nothing else changes. On a direct upgrade from 0.2.0 or earlier, run the steps below first, next to `compose.homelab.yaml`: they keep a key file you already have, move the key from `.env` when there is no file, and make a new key only when neither has one.
- Quickstart (`compose.yaml`): the key now comes from the file `secrets/secrets_key`, never from `.env`. These steps keep a key file you already have, move the value from `.env` when there is no file, and make a new key only when neither has one. A key in `.env` that names another variable, such as `${BASE_KEY}`, is not copied: the steps say so, and you write its value to the file yourself. Then delete the `OWNHEALTH_SECRETS_KEY=` line from `.env`:

```sh
mkdir -p -m 700 secrets
if test -s secrets/secrets_key; then
  echo "secrets/secrets_key is there: keeping it"
elif v=$(sed -n -E 's/^[[:space:]]*(export[[:space:]]+)?OWNHEALTH_SECRETS_KEY[[:space:]]*=[[:space:]]*//p' .env | tail -n 1 |
    sed -E "s/[[:space:]]+#.*\$//; s/[[:space:]]+\$//; s/^\"(.*)\"\$/\\1/; s/^'(.*)'\$/\\1/") && test -n "$v"; then
  case "$v" in
    *'$'*) echo "OWNHEALTH_SECRETS_KEY in .env names a variable: write its value to secrets/secrets_key by hand, then chmod 644 it" >&2 ;;
    *) printf '%s' "$v" > secrets/secrets_key ;;   # the value Compose reads, without quotes or a comment
  esac
else
  openssl rand -base64 32 | tr -d '\n' > secrets/secrets_key   # neither had a key
fi
chmod 644 secrets/secrets_key
```

If OwnHealth then stops with "does not open ... of the ... profile keys", the key is not the one the install was made with. Nothing was changed: put the original key back in `secrets/secrets_key` (from the old `.env`, or from the copy you keep with the backups) and start again. A new key never opens files already stored.

#### Encrypt the older files

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

#### What else to check

Rule pack 0.2.0 stays, so every profile's `state_hash` stays as it was on 0.4.0. `db/app-role.sql` now revokes CONNECT on the `ownhealth` database from PUBLIC: a role you added yourself, such as a monitoring login, needs `grant connect on database ownhealth to <role>`. The `imaging` and `wearables` profiles and sharing stay off until you turn them on. With the homelab stack, update the `src` checkout first (`git -C src pull`), then update the deployed Caddyfile from `src/deploy/Caddyfile` and keep your local access restrictions: the 0.4.0 file has no share listener on port 8090, so sharing links and public wearable callbacks fail without it. Then run `docker compose up -d --force-recreate caddy` to load it.

## Upgrading to 0.4.0

After the backup, set `admin` and `app` to `drumsergio/ownhealth:0.4.0` and `ocr` to `drumsergio/ownhealth:0.4.0-ocr`; nothing else in the compose files changes. Rule pack 0.2.0 moves every profile's `state_hash` once, so restore a backup made before the upgrade with 0.3.0, as above. For the same reason, `restore-test.sh` on a snapshot made before the upgrade reports every profile's `state_hash` as different; run `./deploy/restic/backup.sh` once after the upgrade, so the restore test checks a 0.4.0 snapshot. A household with a genome starts downloading NCBI's weekly ClinVar file (about 200 MB, no personal data in the request); turn it off in Settings, or set a local file path on a server without internet.

## Upgrading to 0.3.0

Take the new `compose.homelab.yaml` (it adds the `ocr` worker, the inbox folder and the `secrets_key` secret), then before `docker compose up -d`:

```sh
test -s secrets/secrets_key || openssl rand -base64 32 | tr -d '\n' > secrets/secrets_key   # keep a key you have
chmod 644 secrets/secrets_key
mkdir -p inbox && sudo chown 10001:10001 inbox
```

Compose refuses to start without the secret file. Keep a copy of it with the restic password.

Scans a 0.2.0 install kept as pending documents are read by OCR once you turn the `ocr` profile on: the worker queues them when it starts.
