# Oura

**MCP policy:** withheld. Oura readings show on the Wearables page only.

**Withheld by:** the Oura API and MCP Agreement, section 4(d): no Oura API data as input to any AI model or AI platform.

## What the connector does

OwnHealth reads Oura through the Open Wearables sidecar (the `wearables` Compose profile, pinned at
0.11.0). The household registers its own Oura app and puts the client id and secret in
`openwearables.env`. Oura needs a public HTTPS callback for it (`deploy/README.md`). The
`openwearables` poller pulls each connected vendor's day series and daily summaries from the sidecar,
stores each day's answer, then maps the readings
(`server/crates/ownhealth-devices/src/openwearables.rs`).

`data/connectors/providers.yaml` marks Oura `ingest: allow, mcp: withheld`. So the readings are
stored and show on the Wearables page and its REST routes. MCP, the rule-pack markers and the
calculators never read them, and an MCP answer counts the days it left out in
`withheld.vendor_terms` (`server/crates/ownhealth-core/src/providers.rs`).

## The vendor's terms

The Oura API and MCP Agreement took effect on June 8, 2026. It sends any use with a large language
model through Oura's own MCP server. It also bans giving Oura API data to an AI model:

> (d) To the extent you seek to use any Oura Data or User Data in connection with a large language model, you may (unless you are an Aggregator) access and process such Oura Data or User Data solely through the MCP Server subject to the provisions of this Agreement. You shall not use, and shall not permit any third party to use, the Oura API to develop, train, fine-tune, evaluate, prompt, or otherwise provide input or data to any AI Model or AI Platform.

Source: <https://cloud.ouraring.com/legal/api-agreement>

Fetched: 2026-11-25 (plain HTTP)

The same agreement limits how long API data may be kept:

> (m) Unless and until you are authorized to do so by the Company in a separate written agreement or consent, you shall not store or cache any data accessed or obtained through the MCP Server. In addition, Aggregators shall not store or cache any User Data or Oura Data. Subject to the foregoing, User Data or Oura Data and User Data accessed or obtained through the Oura API may not be stored or retained beyond the duration strictly necessary.

Source: <https://cloud.ouraring.com/legal/api-agreement>

Fetched: 2026-11-25 (plain HTTP)

OwnHealth keeps the readings it pulls, to show them on the Wearables page. Read clause (m) and decide
whether that fits your use.

## AI and machine learning

Section 4(d) bans using the Oura API to prompt or give input to any AI model or AI platform. That is
what an agent reading the data through OwnHealth's MCP would do, so Oura stays withheld.
