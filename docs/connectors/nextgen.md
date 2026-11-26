# NextGen Enterprise

**MCP policy:** records. What the connector imports joins the person's record, which MCP reads.

## What the connector does

The NextGen connector reads a person's record from NextGen's shared FHIR server
(`https://fhir.nextgen.com/nge/prod/fhir-api-r4/fhir/r4`) with a SMART on FHIR standalone patient
launch. The household registers its own app on the NextGen API Developer Portal as a confidential
client with a client secret.

- **Registration.** The `nextgen` entry of `data/connectors/connectors.yaml` gives the portal, the
  fixed server and the read and search scopes to tick. No real NextGen account has connected with
  these steps yet (`verified_live: false`).
- **Sign-in, tokens and the daily pull** are the same as for every SMART vendor. Sign-in and tokens use
  the shared OAuth core (`server/crates/ownhealth-core/src/connector_oauth.rs`). The daily pull checks
  the patient, runs the US Core searches and stores one bundle per run
  (`server/crates/ownhealth-ingest-structured/src/smart.rs`). The connector never writes to NextGen.

## The vendor's terms

The NextGen Healthcare Developer Portal Terms of Use open in a dialog on the portal's sign-up page.
One clause names open source software:

> 2.6 Open Source. You warrant that your Apps do not contain any open source software licensed under terms that require the user to make (a) the user’s modifications to the open source freely available in source code form, or (b) any software that the user “combines” with the open source software freely available in source code form.

Source: <https://developer.nextgen.com/account/register>

Fetched: 2026-11-25 (in a real browser: the "Terms of Use" link opens a dialog that needs JavaScript)

OwnHealth is licensed under AGPL-3.0-or-later, a licence of the kind this clause describes. Read the
clause before you register OwnHealth as your app at NextGen. We have not asked NextGen how it applies
to a patient's own self-hosted copy.

Access is personal to the registrant:

> 1.2 Access Sign-up. Subject to Applicable Law, in order to access certain APIs you may be required to provide certain information (such as identification or contact details) as part of the registration process for the APIs, or as part of your continued use of the APIs. You agree to keep all registration information accurate and current. Only the person registering may access and use the APIs.

Source: <https://developer.nextgen.com/account/register>

Fetched: 2026-11-25 (in a real browser)

## AI and machine learning

The terms do not mention AI or machine learning.
