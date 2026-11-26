# Vendor terms

Each OwnHealth connector reads data through a vendor's API, and each vendor has terms for that API.
These pages say what each connector does and quote the vendor's own terms on how API data may be
used, including any rule on AI or machine learning. Each quote has its link and the date we read it.
Read the page before you register an app with the vendor: the registration is yours, so the terms
bind you.

This is our reading of the terms, not legal advice. Terms change. Each page gives the date we read
the text, and the weekly link check (`scripts/cite-links.sh`) reports a link that stops working.

## What the policy means

Every page has one **MCP policy** line:

- **served**: MCP, the rule-pack markers and the calculators read this vendor's data. The quoted
  terms show no ban on AI use, and no ban on passing the data to software the person chooses. A
  **Served for** line says whose agents the reading covers: a person with a `read:wearables` grant on
  a profile can point their own agent at it, so the page says what the terms ask of that grant.
- **withheld**: the data shows on the Wearables page and through its REST routes only. MCP, the
  rule-pack markers and the calculators leave it out, and an MCP answer counts the days it left out
  in `withheld.vendor_terms`. The page names the clause that keeps it withheld.
- **records**: a US health record or claims connector. What it imports joins the person's record,
  which MCP reads like any other record. `data/connectors/providers.yaml` does not name these vendors.
  The page quotes the terms so you can judge them before you connect.

`data/connectors/providers.yaml` holds the policy for wearable providers. A provider moves from
withheld to served only when its page quotes the vendor's terms and they allow it. A test in
`providers.rs` checks four things. Every served provider and every record vendor has a page here.
Each page, and its row in the table below, states the same policy as the files. A withheld page has
a **Withheld by** line and a served page a **Served for** line. Each quote has its link and date.

Some sources carry the person's own data: an Apple Health export, a Garmin or Strava account export,
a LibreView or Dexcom Clarity CSV, Gadgetbridge and the person's own Nightscout. These are that
person's files. API terms
do not reach them, so they have no page here and are served.

## The pages

| Vendor | Connector | MCP policy |
|---|---|---|
| [Epic](epic.md) | SMART on FHIR | records |
| [Oracle Health](oracle.md) | SMART on FHIR | records |
| [athenahealth](athena.md) | SMART on FHIR | records |
| [eClinicalWorks (healow)](healow.md) | SMART on FHIR | records |
| [MEDITECH Expanse](meditech.md) | SMART on FHIR | records |
| [NextGen Enterprise](nextgen.md) | SMART on FHIR | records |
| [VA Lighthouse](va.md) | SMART on FHIR | records |
| [CMS Blue Button 2.0](bluebutton.md) | Medicare claims | records |
| [Oura](oura.md) | Open Wearables | withheld |
| [WHOOP](whoop.md) | Open Wearables | withheld |
| [Polar](polar.md) | Open Wearables | served |
| [Withings](withings.md) | Open Wearables | withheld |
| [Dexcom](dexcom.md) | Dexcom API v3 | withheld |
| [Strava](strava.md) | none: account export only | withheld |
