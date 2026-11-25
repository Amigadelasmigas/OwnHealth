# MEDITECH Expanse

**MCP policy:** records. What the connector imports joins the person's record, which MCP reads.

## What the connector does

The MEDITECH connector reads a person's record from their health care organisation's MEDITECH
Expanse server with a SMART on FHIR standalone patient launch. The household registers its own app
through MEDITECH's Greenfield Workspace as a confidential client with a client secret.

- **Registration.** The `meditech` entry of `data/connectors/connectors.yaml` gives the registration
  page and the read and search scopes to tick. No real MEDITECH account has connected with these steps
  yet (`verified_live: false`).
- **Finding the hospital.** The weekly download starts after a person first opens the US connection wizard.
  The instance downloads MEDITECH's SMART User-access Brands bundle once a week and keeps only each
  organisation's name, endpoint and state. The file is deleted after the run
  (`server/crates/ownhealth-ingest-structured/src/brands.rs`).
- **Sign-in, tokens and the daily pull** are the same as for every SMART vendor. Sign-in and tokens use
  the shared OAuth core (`server/crates/ownhealth-core/src/connector_oauth.rs`). The daily pull checks
  the patient, runs the US Core searches and stores one bundle per run
  (`server/crates/ownhealth-ingest-structured/src/smart.rs`). The connector never writes to MEDITECH.

## The vendor's terms

The MEDITECH API Terms of Use limit selling the API or its data:

> When using the MEDITECH API, you may not (or allow those acting on your behalf to):
>
> a. Sell, lease or sublicense the MEDITECH API or any data derived through the MEDITECH API

Source: <https://home.meditech.com/en/d/restapiresources/pages/apiterms.htm>

Fetched: 2026-11-25 (plain HTTP)

Each health care organisation can add its own terms, and those win:

> The HCO with which you register your app may provide its own Terms of Use (“HCO Terms”). You agree to adhere to the HCO Terms. In the event of a conflict between these Terms and the HCO Terms, the HCO Terms will control.

Source: <https://home.meditech.com/en/d/restapiresources/pages/apiterms.htm>

Fetched: 2026-11-25 (plain HTTP)

## AI and machine learning

MEDITECH's terms do not mention AI or machine learning. They ban selling the data, and OwnHealth
sells nothing. Read your health care organisation's own terms too: they can say more, and they
control.
