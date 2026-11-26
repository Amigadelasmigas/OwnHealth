# Oracle Health

**MCP policy:** records. What the connector imports joins the person's record, which MCP reads.

## What the connector does

The Oracle Health connector reads a person's record from their health system's Oracle Health
Millennium server (formerly Cerner) with a SMART on FHIR standalone patient launch. The household
registers its own app in the Oracle Health code console as a public client: a client id alone, no
secret.

- **Registration.** The `oracle` entry of `data/connectors/connectors.yaml` gives the console address
  and the read and search scopes to tick. Its refresh token stays the same on refresh and ends after
  90 days unused. An access token lasts 570 seconds. Both figures are as Oracle documents them (the
  sources are listed in that entry). No real Oracle Health account has connected with these steps yet
  (`verified_live: false`).
- **Finding the hospital.** The weekly download starts after a person first opens the US connection wizard.
  The instance downloads Oracle's Bundle of Endpoint and Organization pairs once a week and keeps only each
  organisation's name, endpoint and state. Oracle's list has no licence, so the file is deleted after
  each run and none ships in the repository
  (`server/crates/ownhealth-ingest-structured/src/brands.rs`).
- **Sign-in, tokens and the daily pull** are the same as for every SMART vendor. Sign-in and tokens use
  the shared OAuth core (`server/crates/ownhealth-core/src/connector_oauth.rs`). The daily pull checks
  the patient, runs the US Core searches and stores one bundle per run
  (`server/crates/ownhealth-ingest-structured/src/smart.rs`). The connector never writes to Oracle.

## The vendor's terms

On 2026-11-25 the terms' address, <https://code.cerner.com/terms-of-use>, redirected to a PDF that
carries the terms' title but holds a different document. The quotes below come from the copy of the
terms (effective 3/1/24) that the Internet Archive saved from Oracle's own address on 2025-09-13.

The terms limit access to the APIs, not the use of the data a patient reads with them:

> You may not allow, expose or provide a third-party access to the Oracle Health APIs, except as expressly set forth in these Terms, without Oracle’s prior written consent in each instance which may be withheld in Oracle’s sole discretion.

Source: <https://web.archive.org/web/20250913061615/https://www.oracle.com/a/ocom/docs/industries/healthcare/terms-and-conditions-oracle-health-api.pdf>

Fetched: 2026-11-25 (plain HTTP, the Internet Archive's copy of 2025-09-13)

A patient-facing app that the person connects themselves needs no approval from the health system:

> Your Developed App must be approved in writing and registered for use by the applicable Oracle client before Oracle will enable your Developed App within such Oracle client’s environment, excluding direct-to-consumer applications which do not need to be pre-approved by Oracle clients.

Source: <https://web.archive.org/web/20250913061615/https://www.oracle.com/a/ocom/docs/industries/healthcare/terms-and-conditions-oracle-health-api.pdf>

Fetched: 2026-11-25 (plain HTTP, the Internet Archive's copy of 2025-09-13)

## AI and machine learning

The terms do not mention AI or machine learning. The ban on third-party access covers the APIs
themselves: OwnHealth keeps the token on the instance and never lets another program call Oracle with
it. Agents read the imported record through OwnHealth, not the Oracle API.
