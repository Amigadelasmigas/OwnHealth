<p align="center"><img src="https://raw.githubusercontent.com/GeiserX/OwnHealth/main/docs/images/banner.svg" alt="OwnHealth" width="800"></p>

<h1 align="center">OwnHealth</h1>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/github/license/GeiserX/OwnHealth?style=flat-square" alt="License"></a>
</p>

OwnHealth is a self-hosted health data layer you own. It keeps your lab results, medical records from any country, wearable data and genome findings in one place you run yourself. You read it in two ways: a web page, and an MCP server that lets an agent such as Claude Code answer "how is my health right now?" with a source behind every value.

There is no chat agent and no AI model inside OwnHealth. The agent is yours, and it sees only what its token allows.

You run it yourself.

<p align="center">
  <img src="https://raw.githubusercontent.com/GeiserX/OwnHealth/main/docs/images/screenshots/dashboard.png" alt="OwnHealth: the family dashboard in the dark theme, with synthetic data" width="800">
</p>

**Status: 1.0.0.** Upgrades from every 0.x release keep your data: see the [changelog](CHANGELOG.md).

## What it does

In 1.0.0:

- One dashboard for the whole family, one profile per person, with OK, Watch, Out, Stale or No data per body system
- Lab reports read from PDFs into trendable values, each linked to its page and position on the page; anything uncertain waits in a review queue
- Inboxes, so data arrives without an upload: a mailbox of its own (allowed senders go to their profile, everything else waits in quarantine until you release it), a watched folder for a scanner or a synced phone folder, Paperless-ngx, the Android share target and an iOS Shortcut
- Local OCR for scans and photos, in the `drumsergio/ownhealth:<version>-ocr` image and the `ocr` compose profile, on CPU; every value it reads is checked against the page. Without it, scans wait for your own agent to read them over MCP
- FHIR and IPS bundles, C-CDA files, SMART Health Cards and Links, the Apple Health export, Health Auto Export, Gadgetbridge, Nightscout with a CGM summary, and manual entry
- US patient portals (Epic, Oracle Health, athenahealth, eClinicalWorks, MEDITECH, NextGen, VA) through SMART on FHIR, and Medicare claims through CMS Blue Button, each with your own registered vendor app, synced every day
- Wearables: Oura, WHOOP, Polar and Withings through the optional `wearables` profile, and Garmin, Strava, Fitbit, FIT, LibreView and Dexcom Clarity exports; a sync alert when a source stops
- Imaging: hospital CDs imported into Orthanc in the optional `imaging` profile, opened in a sandboxed viewer next to their report
- Genome findings from [OwnGenome](https://github.com/GeiserX/OwnGenome): drug response, carrier status and risk variants, joined with your medications and labs
- A drug check that quotes the CPIC guideline for your genotype, and DPWG text from your genome file, next to your current medicines, supplements and relevant labs
- Medicines with their schedule and prescriber, and supplements with a label photo and your recorded decision
- Calculators with their citation and sourced inputs: eGFR (CKD-EPI 2021), FIB-4, HOMA-IR, SCORE2 and PREVENT, or the reason one does not apply
- The screenings and vaccines due for each person by their country's guideline (Spain and the US), the tests to ask for at the next blood draw, and a private calendar feed
- Family history as a pedigree with its cited patterns, exported as FHIR, and ClinVar changes to the variants you carry, checked every week
- A two-page doctor summary and an International Patient Summary, in Spanish or English, where every value names its source; share it through a SMART Health Link with a PIN and revoke, or as an emergency card
- An MCP server with 10 tools, scoped per person and per token, with a citation behind every answer; connect with a token or with OAuth 2.1 (claude.ai and Claude Code, no client registration)
- Passkeys, a hash-chained audit log, each profile's files under its own key, emergency access after a delay the owner can cancel, a full export, and restic backups with a scripted restore test
- Dark theme, Spanish and English, and German, French, Catalan, Italian and Portuguese as beta languages

Limits in 1.0.0. Lab PDFs are read with generic table rules plus one template, for the fictional LabNorte lab. Real lab layouts come later as contributed templates. The LOINC set is a curated subset of 202 codes, so a test outside it waits in review. OCR rows go to review unless a vendor template matches. Only Polar wearable data reach agents; the other vendors stay withheld until their terms allow it ([docs/connectors](docs/connectors/README.md)). No US vendor is verified live yet.

Later: EU formats, a connector SDK and a companion app. See the [roadmap](docs/ROADMAP.md).

## Quick start

You need Docker with Compose v2. OwnHealth runs as one app container, a single Rust binary with the web UI built in (a 364 MB image), next to PostgreSQL 17. OCR is optional: the `ocr` profile adds a worker from the `-ocr` image (436 MB), see [deploy/README.md](deploy/README.md).

```sh
git clone https://github.com/GeiserX/OwnHealth.git
cd OwnHealth
cp .env.example .env
openssl rand -hex 32     # paste as OWNHEALTH_DB_PASSWORD in .env
openssl rand -hex 32     # paste as OWNHEALTH_APP_DB_PASSWORD in .env
openssl rand -hex 48     # paste as OWNHEALTH_SECRET_KEY in .env
mkdir -m 700 secrets && openssl rand -base64 32 | tr -d '\n' > secrets/secrets_key && chmod 644 secrets/secrets_key
                         # seals every profile's files and inbox secrets: keep a copy with your backups
mkdir -p inbox && sudo chown 10001:10001 inbox   # the watched folder, writable by the app
mkdir -p -m 700 keyring/ownhealth && sudo chown 10001:10001 keyring/ownhealth   # the profile keys: never version this folder
docker compose up -d
```

Open http://localhost:8080, create your account with a passkey, and import a lab report under **Import & review**. To have reports arrive by themselves, add a mailbox, the folder, Paperless-ngx or a phone under **Inbox**; the [recipes](docs/recipes/README.md) cover each one.

Connect Claude Code with a token from **MCP & access**:

```sh
export OWNHEALTH_PAT=oh_pat_...   # the token you copied
claude mcp add --transport http ownhealth http://localhost:8080/mcp \
  --header "Authorization: Bearer ${OWNHEALTH_PAT}"
```

The app listens on `127.0.0.1` only. For HTTPS on your network, backups and restores, see [deploy/README.md](deploy/README.md). The full walk-through, with a demo family to look around first, is in [docs/quickstart.md](docs/quickstart.md).

## Intended purpose

OwnHealth is for personal record keeping and for showing values computed from published formulas, each with its source. It is information, not diagnosis. It does not tell anyone what to take or stop; pharmacogenomic results quote the published guideline and link it.

## Documentation

- [Quick start](docs/quickstart.md) and [deploy, back up, restore](deploy/README.md)
- [MCP tool reference](docs/mcp-tools.md): every tool, with example responses
- [Recipes](docs/recipes/README.md) for the inboxes and the claude.ai connector, and [agent-assisted extraction](docs/agent-extraction.md)
- [Plan](docs/PLAN.md): the problem, principles and decisions
- [Roadmap](docs/ROADMAP.md): phases, deliverables and exit criteria
- [Mockup](docs/mockup/README.md): every screen with synthetic data
- Design: [architecture](docs/design/architecture.md), [ingestion](docs/design/ingestion.md), [connectors](docs/design/connectors.md), [MCP](docs/design/mcp.md), [genomics](docs/design/genomics.md), [security, privacy and threat model](docs/design/security-privacy.md)
- [Rivals](docs/research/rivals.md)
- [Contributing](CONTRIBUTING.md): how to add a lab template or an alias
- [Guideline files](docs/guidelines.md): how to add a country's screenings and vaccines
- [Security policy](SECURITY.md)

## Related projects

[OwnGenome](https://github.com/GeiserX/OwnGenome) produces the genome findings OwnHealth shows. It turns a whole genome or a consumer chip into drug-response, carrier and risk findings; OwnHealth ingests them and joins them with your medications and labs.

## License

[AGPL-3.0-or-later](LICENSE)
