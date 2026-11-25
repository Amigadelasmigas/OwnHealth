# Withings

**MCP policy:** withheld. Withings readings show on the Wearables page only.

**Withheld by:** the Withings partner terms, which Withings shows only at registration. They govern the account of every household member other than the person who registered the app, and the policy cannot tell one account from another.

## What the connector does

OwnHealth reads Withings through the Open Wearables sidecar (the `wearables` Compose profile, pinned
at 0.11.0). The household registers its own app on the Withings developer portal and puts the client
id and secret in `openwearables.env`. Withings needs a public HTTPS callback for it
(`deploy/README.md`). The `openwearables` poller pulls each connected vendor's day series and daily
summaries from the sidecar, stores each day's answer, then maps the readings
(`server/crates/ownhealth-devices/src/openwearables.rs`).

`data/connectors/providers.yaml` marks Withings `ingest: allow, mcp: withheld`. So the readings show
on the Wearables page and its REST routes. MCP, the rule-pack markers and the calculators never read
them, and an MCP answer counts the days it left out in `withheld.vendor_terms`
(`server/crates/ownhealth-core/src/providers.rs`).

## The vendor's terms

The Withings General Terms of Use were last updated on 13 October 2026. Their section 6.4, "Sharing
Your Data with Third-Party Services", names the public API and AI assistants as two ways a member may
share their data:

> As a Withings+ User, you can connect your Account to third-party services so that they can access the data you choose to share. This connection can be established in two ways: the Withings Public API, which allows a third-party application to receive your data, and the Withings MCP server (also known as the Withings AI connector), which allows an AI-based assistant to query your data at your request.

Source: <https://www.withings.com/en-us/policies/terms-of-service>

Fetched: 2026-11-25 (in a real browser: plain HTTP answers 403)

The same section says which terms apply to a person who builds a tool for their own use:

> This article pertains to your use of your own data, whether you are connecting an existing service to your Account or developing a tool yourself for your own personal use. If you develop an application intended for other users, or if you use the data obtained for purposes other than your own needs, the terms applicable to partners apply and take precedence over this article; they are available on the Withings developer portal.

Source: <https://www.withings.com/en-us/policies/terms-of-service>

Fetched: 2026-11-25 (in a real browser)

Section 6.4.1 adds that sharing with third-party services needs a Withings+ subscription, with some
exceptions.

The partner terms are shown at registration on the Withings developer portal. The latest public copy
we found is the API Terms of Use of 24 April 2023 ("Version du 24 avril 2023", with the text in
English). Its additional terms list what a developer may not do, including:

> (ii) collect User Content without the express prior consent of the user;

Source: <https://media.withings.com/web/legal/legal-pdf/api-terms-of-use/wo-fr-api-terms-of-use-20230424.pdf>

Fetched: 2026-11-25 (plain HTTP)

> (v) use the Withings API(s) to issue queries that are not in response to end user actions; or

Source: <https://media.withings.com/web/legal/legal-pdf/api-terms-of-use/wo-fr-api-terms-of-use-20230424.pdf>

Fetched: 2026-11-25 (plain HTTP)

Open Wearables polls on a schedule after the member connects, not in response to each action of the
member. Read item (v) and the partner terms shown at registration, and decide whether that fits your
use.

## AI and machine learning

Neither quoted text bans AI or machine learning. Section 6.4 names an AI-based assistant that queries
the member's data at their request as one way to share it. That covers a member who reads their own
account with a tool built for their own use. It does not cover every account the household connects:

- **One app for the household.** One person registers the Withings app, and every member connects
  through it (`openwearables.env` holds one client per household). When another member connects
  their own Withings account, the app serves another user. Then the partner terms shown at
  registration apply and take precedence over 6.4. We cannot quote them, and their last public copy
  bans queries that are not in response to end user actions (item (v) above).
- **One policy for every account.** `providers.yaml` serves or withholds a provider, not one account
  (`Reader::sees` in `server/crates/ownhealth-core/src/providers.rs`). Serving Withings would serve
  every member's account, so Withings stays withheld for all of them.

To serve Withings, the person who registered the app reads the partner terms shown at registration.
If they show no ban on AI processing, no ban on passing the data to software the member chooses and
no ban on scheduled queries, quote them on this page, set the `withings` line to `mcp: served` in
`data/connectors/providers.yaml`, and replace the **Withheld by** line with a **Served for** line.
