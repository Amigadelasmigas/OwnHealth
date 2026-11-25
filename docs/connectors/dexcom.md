# Dexcom

**MCP policy:** withheld. Readings from the Dexcom API would show on the Wearables page only.

**Withheld by:** the Dexcom Data Licensing Agreement and Registered Developer Agreement. Dexcom shows them only at registration, so no public text can be quoted to clear them.

## What the connector does

A native connector to the Dexcom API v3 is planned but not built yet. OwnHealth only reserves its
kind: `dexcom` is one of the OAuth kinds (`OAUTH_KINDS` in
`server/crates/ownhealth-core/src/connector_oauth.rs`) and of the connector kinds (`CONNECTOR_KINDS`
in `server/crates/ownhealth-core/src/sources.rs`).

`data/connectors/providers.yaml` marks the provider `dexcom` as `ingest: allow, mcp: withheld`. So
readings it stores would show on the Wearables page and its REST routes. MCP, the rule-pack markers
and the calculators would never read them, and an MCP answer would count the days it left out in
`withheld.vendor_terms` (`server/crates/ownhealth-core/src/providers.rs`).

Two other paths carry Dexcom sensor readings. They are served because they are the person's own
files: a Dexcom Clarity CSV export (provider `dexcom_clarity`, read by
`server/crates/ownhealth-devices/src/cgm_csv.rs`) and the person's own Nightscout (provider
`nightscout`). The API terms below do not reach them.

## The vendor's terms

Dexcom's developer documentation says that the data licence comes only when Dexcom approves an app,
and that it is standard and non-negotiable:

> If the team decides to authorize your request, you will be sent a standard, non-negotiable Data Licensing Agreement.

Source: <https://developer.dexcom.com/docs/dexcom/scopes-access>

Fetched: 2026-11-25 (in a real browser: plain HTTP returns a page that needs JavaScript)

The developer agreement is accepted during registration:

> Developer information is collected during the registration process, including name, email, username, password, and acceptance of the Registered Developer Agreement.

Source: <https://developer.dexcom.com/docs/dexcom/scopes-access>

Fetched: 2026-11-25 (in a real browser)

Both agreements are shown at registration. We found no public copy of either.

## AI and machine learning

The public documentation does not say whether the agreements allow AI processing or passing the data
to software the person chooses. Until a public text shows that, the `dexcom` provider stays
withheld.
