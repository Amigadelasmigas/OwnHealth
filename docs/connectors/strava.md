# Strava

**MCP policy:** withheld. OwnHealth stores nothing from the Strava API.

**Withheld by:** the Strava API Policy. Section 5.3 bans Strava Data in the operation of any AI Application, and section 5.16 (b) bans an MCP server that exposes Strava Data.

## What the connector does

OwnHealth has no Strava API connector. `data/connectors/providers.yaml` marks the provider `strava`
as `ingest: drop, mcp: withheld`, so the sample store refuses any write from it
(`server/crates/ownhealth-core/src/providers.rs`).

Strava data comes in only as the person's own bulk account export. The importer is
`server/crates/ownhealth-devices/src/strava_zip.rs`, and its provider, `strava_export`, is served:
the export is the person's own file, which the API terms do not reach.

## The vendor's terms

The Strava API Policy (effective date: June 1, 2026), which forms part of the Strava API Agreement,
bans the use of Strava Data with AI in section 5.3:

> You may not use the Strava API Materials or Strava Data, directly or indirectly, in connection with the development, training, evaluation, or operation of any AI Application.

Source: <https://www.strava.com/legal/api_policy>

Fetched: 2026-11-25 (plain HTTP)

Section 5.16 bans MCP servers. Under it, a developer may not:

> (b) operate any MCP Server, agent-mediated interface, or analogous mechanism that exposes the Strava API Materials, Strava Data, or any subset thereof;

Source: <https://www.strava.com/legal/api_policy>

Fetched: 2026-11-25 (plain HTTP)

## AI and machine learning

Section 5.3 bans the operation of any AI application with Strava Data, and section 5.16 names the
Strava MCP as the only agent interface allowed. An OwnHealth MCP answer that held Strava API data
would break both. So OwnHealth does not store the API's data at all.
