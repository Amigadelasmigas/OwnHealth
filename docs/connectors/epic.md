# Epic

**MCP policy:** records. What the connector imports joins the person's record, which MCP reads.

## What the connector does

The Epic connector reads a person's record from their health system's Epic server (MyChart) with a
SMART on FHIR standalone patient launch. The household registers its own app at Epic, as a
confidential client with a client secret. No project client exists, and no secret ships in the repository.

- **Registration.** The US connection wizard shows the redirect URI, the credential and the scopes to
  tick, from the `epic` entry of `data/connectors/connectors.yaml`. The scopes are read and search
  only (`patient/<type>.rs`), plus `openid fhirUser launch/patient offline_access`. No real Epic
  account has connected with these steps yet (`verified_live: false`).
- **Finding the hospital.** The weekly download starts after a person first opens the wizard. The instance
  downloads Epic's SMART User-access Brands bundle once a week and keeps only each organisation's
  name, endpoint and state, in the `fhir_brand` table. The downloaded file is deleted after the run,
  and no Epic file ships in the repository (`server/crates/ownhealth-ingest-structured/src/brands.rs`).
- **Sign-in and tokens.** One shared OAuth core builds the sign-in, keeps the refresh token sealed and
  refreshes it under a row lock (`server/crates/ownhealth-core/src/connector_oauth.rs`,
  `server/crates/ownhealth-api/src/routes/connectors.rs`).
- **The daily pull.** Once a day the pull reads the Patient and checks the name and full birth date
  against the profile. Then it runs the US Core searches (allergies, conditions, reports, documents,
  encounters, immunizations, medicine requests, observations and procedures). Each run stores one
  bundle and imports only new resources (`server/crates/ownhealth-ingest-structured/src/smart.rs`).
  The connector never writes to Epic.

## The vendor's terms

The open.epic Terms of Use govern developers who are not Epic customers. They leave the use of the
data to the developer:

> Epic provides the Materials for your use, and you decide how to use them to interoperate with Epic software and provide your products and services to Epic Community Members and end users. With this freedom comes the responsibility to make sure your products and services are behaving appropriately.

Source: <https://fhir.epic.com/Resources/Terms>

Fetched: 2026-11-25 (plain HTTP. The page says "Last updated: October 31, 2025")

The same terms allow sharing Epic's materials by link only. This is why OwnHealth ships no copy of
Epic's endpoint list and downloads it on each instance:

> This website is the source of truth for current versions of the Materials. You can keep copies of the Materials for yourself. You may distribute them only by linking others directly to the Materials on the website.

Source: <https://fhir.epic.com/Resources/Terms>

Fetched: 2026-11-25 (plain HTTP)

## AI and machine learning

The open.epic Terms of Use do not mention AI or machine learning. They place no limit on passing the
person's data to software the person chooses. The health system that runs the Epic server can have
its own rules for apps. The wizard cannot see them.
