# CMS Blue Button 2.0

**MCP policy:** records. What the person accepts from the import joins their record, which MCP reads.

## What the connector does

The Blue Button connector reads a Medicare beneficiary's Part A, B and D claims from CMS's Blue
Button v3 API. The household registers its own app at CMS as a confidential client. The sandbox is
self-serve. Production needs CMS's review.

- **Registration.** The `bluebutton` entry of `data/connectors/connectors.yaml` gives the sandbox
  and production servers and the scopes to tick: `launch/patient` and read and search of Patient,
  Coverage and ExplanationOfBenefit. A refresh token is single-use: once sent, it is spent even when
  no answer comes back. No real Medicare account has connected with these steps yet
  (`verified_live: false`).
- **Sign-in and tokens** use the shared OAuth core (`server/crates/ownhealth-core/src/connector_oauth.rs`).
- **The pull.** The grant names one Patient, whose details must match the profile. Coverage and
  ExplanationOfBenefit are kept as FHIR in one sealed collection and go to the grouped import review,
  where the person accepts or rejects them (`server/crates/ownhealth-ingest-structured/src/bluebutton.rs`).
  The connector never writes to CMS.

## The vendor's terms

The Blue Button API Terms of Service (current version v3, published 11/1/2022) let an app disclose a
beneficiary's data only with that person's consent:

> You further acknowledge that when records regarding an individual are obtained through a Blue Button API, you may not disclose any information or data regarding the individual to any other individuals or third parties without specific, explicit consent from the individual or his or her authorized representative.

Source: <https://bluebutton.cms.gov/terms/>

Fetched: 2026-11-25 (plain HTTP)

Credentials stay out of open source code. OwnHealth ships none: each household enters its own client
id and secret in the wizard.

> Developer credentials (such as passwords, keys, tokens, and client IDs) issued to you by CMS are intended to be used only by you and to identify any software which you are using with the Blue Button APIs. You agree to keep your developer credentials confidential and make reasonable efforts to prevent and discourage other persons or entities from accessing or using your developer credentials. Developer credentials may not be embedded in open source projects.

Source: <https://bluebutton.cms.gov/terms/>

Fetched: 2026-11-25 (plain HTTP)

Every app must show a notice:

> All services that utilize or access the Blue Button APIs or content, data, documentation, code, and related materials associated with the Blue Button APIs must display the following notice prominently within the application:
>
> “This product uses the Blue Button APIs but is not endorsed or certified by the Centers for Medicare & Medicaid Services or the U.S. Department of Health and Human Services.”

Source: <https://bluebutton.cms.gov/terms/>

Fetched: 2026-11-25 (plain HTTP)

OwnHealth does not show this notice in the app yet. It is an open item for the Blue Button
connection screens.

## AI and machine learning

The terms do not mention AI or machine learning. Our reading: an agent that the beneficiary connects
to their own instance gets the data with the beneficiary's explicit consent, which the first clause
allows.
