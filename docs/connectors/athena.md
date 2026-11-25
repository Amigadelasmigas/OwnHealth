# athenahealth

**MCP policy:** records. What the connector imports joins the person's record, which MCP reads.

## What the connector does

The athenahealth connector reads a person's record from athenahealth's shared FHIR server
(`https://api.platform.athenahealth.com/fhir/r4`) with a SMART on FHIR standalone patient launch. The
household registers its own app as a confidential client with a client secret.

- **Registration.** The `athena` entry of `data/connectors/connectors.yaml` gives the registration
  page, the fixed server and the read and search scopes to tick. The refresh token ends after 90
  days unused. No real athenahealth account has connected with these steps yet
  (`verified_live: false`).
- **Sign-in, tokens and the daily pull** are the same as for every SMART vendor. Sign-in and tokens use
  the shared OAuth core (`server/crates/ownhealth-core/src/connector_oauth.rs`). The daily pull checks
  the patient, runs the US Core searches and stores one bundle per run
  (`server/crates/ownhealth-ingest-structured/src/smart.rs`). The connector never writes to
  athenahealth.

## The vendor's terms

The athenahealth API terms grant use of the API and its materials for an integration with athenaOne:

> Subject to these Terms, Athena grants you a limited, non-exclusive, revocable, non-transferable, and non-sublicensable right to access and use the Developer Portal, API and Athena-owned content, documentation, code, and related materials made available to you on or through the Developer Portal or API (collectively, the “Materials”) in connection with the integration of your products and/or services with athenaOne.

Source: <https://docs.athenahealth.com/api/guides/termsofservice>

Fetched: 2026-11-25 (through the r.jina.ai reader: the site answered 403 to plain HTTP and to a real browser from our addresses)

Among the things a developer will not do, the terms list reselling access, not using the data:

> sell, rent, lease, sublicense, redistribute, or syndicate access to the API, athenaOne, or Materials to any third party without prior written approval from Athena;

Source: <https://docs.athenahealth.com/api/guides/termsofservice>

Fetched: 2026-11-25 (through the r.jina.ai reader)

## AI and machine learning

The terms do not mention AI or machine learning. The ban on resale covers access to the API and
athenaOne. OwnHealth keeps the token on the instance, and agents read the imported record through
OwnHealth, not the athenahealth API.
