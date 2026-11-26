# eClinicalWorks (healow)

**MCP policy:** records. What the connector imports joins the person's record, which MCP reads.

## What the connector does

The healow connector reads a person's record from a practice that runs eClinicalWorks, through the
healow patient FHIR API, with a SMART on FHIR standalone patient launch. The household registers its
own app on the healow developer portal as a confidential client with a client secret.

- **Registration.** The `healow` entry of `data/connectors/connectors.yaml` gives the portal and the
  read and search scopes to tick. Each refresh brings a new refresh token and retires the old one. No
  real healow account has connected with these steps yet (`verified_live: false`).
- **Finding the practice.** The weekly download starts after a person first opens the US connection wizard.
  The instance downloads healow's Bundle of Endpoint and Organization pairs once a week and keeps only each
  practice's name, endpoint and state. The file is deleted after the run
  (`server/crates/ownhealth-ingest-structured/src/brands.rs`).
- **Sign-in, tokens and the daily pull** are the same as for every SMART vendor. Sign-in and tokens use
  the shared OAuth core (`server/crates/ownhealth-core/src/connector_oauth.rs`). The daily pull checks
  the patient, runs the US Core searches and stores one bundle per run
  (`server/crates/ownhealth-ingest-structured/src/smart.rs`). The connector never writes to healow.

## The vendor's terms

The healow FHIR API Terms of Service for app developers are shown on the portal's Getting Started
page (the tile "FHIR APIs Terms of Service for App Developers"). Read three of them before you
register.

Storage needs the patient's authorization:

> (xii) except as authorized by an End User about whom such data pertains, maintain a cache or store using any kind of storage of any set or subset of data accessed using the healow Materials;

Source: <https://connect4.healow.com/apps/jsp/dev/r4/gettingStarted.jsp>

Fetched: 2026-11-25 (in a real browser: the terms open in a dialog that needs JavaScript)

The data may go to End Users and to no other third party:

> 9.6. You will not copy, store, archive, distribute to any third party (other than to End Users as contemplated in this Agreement), alter, or modify in any fashion, any healow Data, any metadata or any Link.

Source: <https://connect4.healow.com/apps/jsp/dev/r4/gettingStarted.jsp>

Fetched: 2026-11-25 (in a real browser)

The app and its data stay in the United States:

> 9.17. The Developer Application will only be accessible in the United States. You will not store data outside the United States.

Source: <https://connect4.healow.com/apps/jsp/dev/r4/gettingStarted.jsp>

Fetched: 2026-11-25 (in a real browser)

Our reading: a person who connects their own record to their own instance is the End User who
authorizes the storage. The terms do not say whether an agent that the same person connects to the
instance is a third party. Clause 9.17 rules out an instance that runs or stores data outside the
United States.

## AI and machine learning

The terms do not mention AI or machine learning.
