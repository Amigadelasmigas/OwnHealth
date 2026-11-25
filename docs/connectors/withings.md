# Withings

**MCP policy:** served. Withings readings reach MCP, the rule-pack markers and the calculators.

**Served for:** the member's own agent, and the agent of a person the member gives a `read:wearables` grant (section 6.4: the member's own data, for their own needs). When another household member's Withings account connects through an app someone else registered, the partner terms shown at registration apply to that account instead. Read them before that member connects.

## What the connector does

OwnHealth reads Withings through the Open Wearables sidecar (the `wearables` Compose profile, pinned
at 0.11.0). The household registers its own app on the Withings developer portal and puts the client
id and secret in `openwearables.env`. Withings needs a public HTTPS callback for it
(`deploy/README.md`). The `openwearables` poller pulls each connected vendor's day series and daily
summaries from the sidecar, stores each day's answer, then maps the readings
(`server/crates/ownhealth-devices/src/openwearables.rs`).

`data/connectors/providers.yaml` marks Withings `ingest: allow, mcp: served`. So the readings show on
the Wearables page, and MCP, the rule-pack markers and the calculators read them like any other
served source (`server/crates/ownhealth-core/src/providers.rs`).

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

Open Wearables polls on a schedule after the member connects. Read item (v) and the partner terms
shown at registration, and decide whether that fits your use.

## AI and machine learning

Neither text bans AI or machine learning. Section 6.4 names an AI-based assistant that queries the
member's data at their request as one way to share it. So Withings is served, on this reading:

- **The member's own agent.** A member connects their own Withings account and points their own
  agent at their own profile. That is the use section 6.4 describes.
- **Another person's agent.** A person with a grant on the member's profile that includes
  `read:wearables` can create an MCP token for that profile, and their agent then reads the
  readings. The grant is the member's choice, so give it only to someone whose agent the member
  agrees may read them. Item (ii) of the 2023 terms asks for the user's express prior consent.
- **One app for the household.** One person registers the Withings app, and every member connects
  through it (`openwearables.env` holds one client per household). Section 6.4 covers that person
  reading their own account with a tool built for their own use. When another household member
  connects their own Withings account through the app, the app serves another user. Then the partner
  terms shown at registration apply and take precedence over 6.4. Read them before another member
  connects.
