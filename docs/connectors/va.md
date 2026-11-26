# VA Lighthouse

**MCP policy:** records. What the connector imports joins the person's record, which MCP reads.

## What the connector does

The VA connector reads a Veteran's record from the VA Lighthouse Patient Health API
(`https://api.va.gov/services/fhir/v0/r4`) with a SMART on FHIR standalone patient launch. The
household registers its own app at the VA developer site as a confidential client with a client
secret.

- **Registration.** The `va` entry of `data/connectors/connectors.yaml` gives the registration page,
  the fixed server and the scopes to tick. The VA runs SMART 1.0, so the scopes are `.read`, not
  `.rs`. In production the refresh token ends after 42 days unused. No real VA account has connected
  with these steps yet (`verified_live: false`).
- **Sign-in, tokens and the daily pull** are the same as for every SMART vendor. Sign-in and tokens use
  the shared OAuth core (`server/crates/ownhealth-core/src/connector_oauth.rs`). The daily pull checks
  the patient, runs the US Core searches and stores one bundle per run
  (`server/crates/ownhealth-ingest-structured/src/smart.rs`). The connector never writes to the VA.

## The vendor's terms

The VA API Terms of Service let an app share a Veteran's records only with that person's consent:

> You further acknowledge that when records regarding an individual are obtained through a VA API, you may not expose that content to other individuals or third parties without specific, explicit consent from the individual or his or her authorized representative, or as permitted by applicable law.

Source: <https://developer.va.gov/terms-of-service>

Fetched: 2026-11-25 (in a real browser: plain HTTP returns a page that needs JavaScript. The terms say "Last Updated: March 14, 2024")

They also keep credentials out of open source code. OwnHealth ships none: each household enters its
own client id and secret in the wizard.

> Developer credentials (such as passwords, keys, tokens, and client IDs) are intended to be used only by you and identify any software which you are using with the APIs. You will keep your credentials confidential and make reasonable efforts to prevent and discourage other API Clients from using your credentials. Developer credentials may not be embedded in open source projects.

Source: <https://developer.va.gov/terms-of-service>

Fetched: 2026-11-25 (in a real browser)

## AI and machine learning

The terms do not mention AI or machine learning. Our reading: an agent that the Veteran connects to
their own instance reads their records with their explicit consent, which the first clause allows.
