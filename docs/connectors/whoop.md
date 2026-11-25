# WHOOP

**MCP policy:** withheld. WHOOP readings show on the Wearables page only.

**Withheld by:** the WHOOP API Terms of Use, section 4 (Prohibitions on WHOOP Data): no WHOOP Data used to create or develop any AI application or system.

## What the connector does

OwnHealth reads WHOOP through the Open Wearables sidecar (the `wearables` Compose profile, pinned at
0.11.0). The household registers its own WHOOP app and puts the client id and secret in
`openwearables.env` (`deploy/README.md`). The `openwearables` poller pulls each connected vendor's
day series and daily summaries from the sidecar, stores each day's answer, then maps the readings
(`server/crates/ownhealth-devices/src/openwearables.rs`).

`data/connectors/providers.yaml` marks WHOOP `ingest: allow, mcp: withheld`. So the readings are
stored and show on the Wearables page and its REST routes. MCP, the rule-pack markers and the
calculators never read them, and an MCP answer counts the days it left out in
`withheld.vendor_terms` (`server/crates/ownhealth-core/src/providers.rs`).

## The vendor's terms

The WHOOP API Terms of Use (effective date: October 6, 2026) open their list of prohibitions on WHOOP
Data this way:

> Prohibitions on WHOOP Data. Unless expressly authorized by the WHOOP Data owner or by applicable law, Company will not, and will not permit Company’s End Users or others acting on Company’s behalf to, do any of the following:

Source: <https://developer.whoop.com/api-terms-of-use/>

Fetched: 2026-11-25 (plain HTTP)

One item on that list is AI:

> Use any API or WHOOP Data to create, develop, test, train, fine-tune or improve any large language model or any other machine learning or artificial intelligence model, application, or system;

Source: <https://developer.whoop.com/api-terms-of-use/>

Fetched: 2026-11-25 (plain HTTP)

Another limits copies:

> Scrape, build databases, or otherwise create permanent copies of WHOOP Data, or keep cached copies longer than permitted by the cache header;

Source: <https://developer.whoop.com/api-terms-of-use/>

Fetched: 2026-11-25 (plain HTTP)

OwnHealth keeps the readings it pulls, to show them on the Wearables page. Read this item and decide
whether that fits your use.

## AI and machine learning

The AI item covers any "application, or system", and training is only one of its verbs. Serving WHOOP Data to an agent
through OwnHealth's MCP can count as developing an AI application with it. The list allows what the
"WHOOP Data owner" authorizes, but the terms do not say whether the member is that owner. So WHOOP
stays withheld.
