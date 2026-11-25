# Polar

**MCP policy:** served. Polar readings reach MCP, the rule-pack markers and the calculators.

## What the connector does

OwnHealth reads Polar through the Open Wearables sidecar (the `wearables` Compose profile, pinned at
0.11.0). The household registers its own client in Polar's AccessLink admin and puts the client id
and secret in `openwearables.env` (`deploy/README.md`). The `openwearables` poller pulls each
connected vendor's day series and daily summaries from the sidecar, stores each day's answer, then
maps the readings (`server/crates/ownhealth-devices/src/openwearables.rs`).

`data/connectors/providers.yaml` marks Polar `ingest: allow, mcp: served`. So the readings show on
the Wearables page, and MCP, the rule-pack markers and the calculators read them like any other
served source (`server/crates/ownhealth-core/src/providers.rs`).

## The vendor's terms

The AccessLink API documentation names its licence and links it to a public page:

> License: Polar Electro AccessLink Limited License Agreement

Source: <https://www.polar.com/accesslink-api/>

Fetched: 2026-11-25 (plain HTTP)

Section 3.1 of that agreement lists what the developer shall do. The first item lets data go where
the member explicitly allows:

> 3.1.1 shall transmit, receive and use Data only pursuant to the Member’s explicit authorization (as applicable) and in accordance with any requirements of applicable law (including but not limited, if applicable to Your Software, General Data Protection Regulation of the European Union), ensuring that no Data is distributed to external sources without the explicit Member permission (as applicable);

Source: <https://www.polar.com/en/legal/polar-api-agreement>

Fetched: 2026-11-25 (plain HTTP. The agreement says "Latest update: 22 August 2025")

Another item asks apps to name Polar as the source:

> 3.1.5 shall credit Polar Ecosystem as the source of Data when displaying any Data, or any data derived from Data in Your application or service, as further instructed in Polar Brand Guidelines;

Source: <https://www.polar.com/en/legal/polar-api-agreement>

Fetched: 2026-11-25 (plain HTTP)

## AI and machine learning

The agreement does not mention AI or machine learning. Our reading: the member connects their own
agent to their own instance, which is an explicit authorization under 3.1.1. So Polar is served.
