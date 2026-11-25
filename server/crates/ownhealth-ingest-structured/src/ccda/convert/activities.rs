//! C-CDA Encounter Activities, procedure activities and Social History observations.

use roxmltree::Node;
use serde_json::{Map, Value, json};

use super::{
    Conv, Section, SectionReport, attr, attr_ts, child, children, coded, describe, digits, elements, has_template,
    is_security_observation, is_true, lookup, ordered_period, path, put, status_of, templates, xml,
};

const ENCOUNTER_ACTIVITY: &str = "2.16.840.1.113883.10.20.22.4.49";
const INDICATION: &str = "2.16.840.1.113883.10.20.22.4.19";
const PROCEDURE_ACT: &str = "2.16.840.1.113883.10.20.22.4.12";
const PROCEDURE_OBSERVATION: &str = "2.16.840.1.113883.10.20.22.4.13";
const PROCEDURE_ACTIVITY: &str = "2.16.840.1.113883.10.20.22.4.14";
const NOTE_ACTIVITY: &str = "2.16.840.1.113883.10.20.22.4.202";
// These templates wait in review even when they also declare a supported Social History root.
const EXCLUDED_SOCIAL_OBSERVATIONS: &[&str] = &[
    "2.16.840.1.113883.10.20.22.4.501", // Sexual Orientation
    "2.16.840.1.113883.10.20.15.3.8",   // Pregnancy Status
    "2.16.840.1.113883.10.20.22.4.281", // Pregnancy Intention in Next Year
    "2.16.840.1.113883.10.20.15.3.1",   // Estimated Date of Delivery
];
const SOCIAL_OBSERVATIONS: &[&str] = &[
    "2.16.840.1.113883.10.20.22.4.38", // Social History Observation
    "2.16.840.1.113883.10.20.22.4.38.1",
    "2.16.840.1.113883.10.20.22.4.78", // Smoking Status
    "2.16.840.1.113883.10.20.22.4.78.2",
    "2.16.840.1.113883.10.20.22.4.85", // Tobacco Use
    "2.16.840.1.113883.10.20.22.4.85.2",
    "2.16.840.1.113883.10.20.22.4.200", // Birth Sex
    "2.16.840.1.113883.10.20.22.4.503", // Basic Occupation
    "2.16.840.1.113883.10.20.22.4.504", // Basic Industry
    "2.16.840.1.113883.10.20.22.4.506", // Tribal Affiliation
    "2.16.840.1.113883.10.20.22.4.507", // Sex
    "2.16.840.1.113883.10.20.22.4.511", // Smoking Status (new template)
];

pub(super) fn is_note_activity(n: Node<'_, '_>) -> bool {
    n.has_tag_name((xml::NS, "act")) && has_template(n, &[NOTE_ACTIVITY])
}

fn supported(n: Node<'_, '_>, roots: &[&str]) -> bool {
    let ids = templates(n);
    ids.is_empty() || ids.iter().any(|id| roots.contains(id))
}

fn event(n: Node<'_, '_>, roots: &[&str]) -> bool {
    supported(n, roots) && attr(n, "moodCode") == Some("EVN") && attr(n, "nullFlavor").is_none()
}

fn act_encounter_class(code: Option<Node<'_, '_>>) -> Value {
    const ACT_CODE_OID: &str = "2.16.840.1.113883.5.4";
    const CPT_OID: &str = "2.16.840.1.113883.6.12";
    const ACT_CODE: &str = "http://terminology.hl7.org/CodeSystem/v3-ActCode";
    const CLASSES: [(&str, &str); 11] = [
        ("AMB", "ambulatory"),
        ("EMER", "emergency"),
        ("FLD", "field"),
        ("HH", "home health"),
        ("IMP", "inpatient encounter"),
        ("ACUTE", "inpatient acute"),
        ("NONAC", "inpatient non-acute"),
        ("OBSENC", "observation encounter"),
        ("PRENC", "pre-admission"),
        ("SS", "short stay"),
        ("VR", "virtual"),
    ];
    let unknown = || json!({"extension": [{"url": "http://hl7.org/fhir/StructureDefinition/data-absent-reason", "valueCode": "unknown"}]});
    let Some(code) = code else {
        return unknown();
    };
    let codings = || std::iter::once(code).chain(children(code, "translation"));
    for c in codings().filter(|c| attr(*c, "codeSystem") == Some(ACT_CODE_OID)) {
        if let Some(value) = attr(c, "code")
            && let Some(display) = lookup(&CLASSES, Some(value))
        {
            return json!({"system": ACT_CODE, "code": value, "display": attr(c, "displayName").unwrap_or(display)});
        }
    }
    for c in codings().filter(|c| attr(*c, "codeSystem") == Some(CPT_OID)) {
        let Some(value) = attr(c, "code")
            .filter(|v| v.len() == 5 && digits(v))
            .and_then(|v| v.parse::<u32>().ok())
        else {
            continue;
        };
        let class = match value {
            99202..=99205 | 99211..=99215 => ("AMB", "ambulatory"),
            99221..=99223 => ("IMP", "inpatient encounter"),
            99281..=99285 => ("EMER", "emergency"),
            _ => continue,
        };
        return json!({"system": ACT_CODE, "code": class.0, "display": class.1});
    }
    unknown()
}

/// Whether a `<text>` holds words of its own. A bare `<reference>` adds none: its words are in the
/// section narrative.
fn has_words(text: Node<'_, '_>) -> bool {
    text.descendants()
        .any(|n| n.is_text() && n.text().is_some_and(|s| !s.trim().is_empty()))
}

/// A Reference that can be read without a separate Practitioner resource. The display is a person's
/// or an organization's name, never the role code ("General physician" names no provider).
fn assigned_reference(entity: Node<'_, '_>) -> Option<Value> {
    let mut reference = Map::new();
    let display = path(entity, &["assignedPerson", "name"])
        .and_then(|n| {
            let parts: Vec<_> = elements(n).filter_map(xml::text).collect();
            if parts.is_empty() {
                xml::text(n)
            } else {
                Some(parts.join(" "))
            }
        })
        .or_else(|| path(entity, &["representedOrganization", "name"]).and_then(xml::text));
    put(&mut reference, "display", display.map(Value::String));
    if let Some(Value::Array(ids)) = Conv::identifiers(&[entity]) {
        put(&mut reference, "identifier", ids.into_iter().next());
    }
    (!reference.is_empty()).then_some(Value::Object(reference))
}

impl Conv<'_, '_> {
    /// Anything other than a note, a security label or a relationship the caller reads waits in review.
    fn unread_relationships(&mut self, n: Node<'_, '_>, rep: &mut SectionReport) {
        for part in children(n, "entryRelationship").flat_map(elements) {
            if !is_note_activity(part) && !is_security_observation(part) {
                let line = self.hold(part);
                rep.not_read.push(line);
            }
        }
    }

    pub(super) fn encounter(&mut self, enc: Node<'_, '_>, at: &str, rep: &mut SectionReport) -> bool {
        if !event(enc, &[ENCOUNTER_ACTIVITY]) || is_true(enc, "negationInd") {
            return false;
        }
        let status = match status_of(enc) {
            Some("active") => "in-progress",
            Some("completed") => "finished",
            Some("aborted" | "cancelled") => "cancelled",
            Some("nullified") => "entered-in-error",
            Some("held" | "suspended") => "onleave",
            Some("new") => "planned",
            _ => "unknown",
        };

        // An indication is why the visit happened: a reason of the Encounter, never a Condition of
        // its own. A Condition would claim a clinical status the visit does not give, beside the one
        // the Problems section gives. Narrative the reason uses labels the Encounter.
        let mut reasons = Vec::new();
        for rel in children(enc, "entryRelationship") {
            for part in elements(rel) {
                if is_note_activity(part) || is_security_observation(part) {
                    continue;
                }
                // "not the reason" (a negated relationship) and a reversed one are no visit reason
                let indication = attr(rel, "typeCode") == Some("RSON")
                    && !is_true(rel, "negationInd")
                    && !is_true(rel, "inversionInd")
                    && part.has_tag_name((xml::NS, "observation"))
                    && event(part, &[INDICATION])
                    && !is_true(part, "negationInd")
                    && !matches!(status_of(part), Some("aborted" | "cancelled" | "nullified"));
                match indication.then(|| self.concept(child(part, "value"))).flatten() {
                    Some(reason) => {
                        reasons.push(reason);
                        // The reason keeps the coded value only: words in the indication's own text
                        // exist only in the file.
                        if children(part, "text").any(has_words) {
                            rep.not_read.push("indication (text kept in the uploaded file)".into());
                        }
                        self.unread_relationships(part, rep);
                    }
                    None => {
                        let line = self.hold(part);
                        rep.not_read.push(line);
                    }
                }
            }
        }

        let code = child(enc, "code");
        let id = self.new_id("Encounter", &[enc], enc);
        let mut r = Map::from_iter([
            ("resourceType".into(), json!("Encounter")),
            ("id".into(), json!(id)),
            ("status".into(), json!(status)),
            ("class".into(), act_encounter_class(code)),
        ]);
        put(&mut r, "identifier", Self::identifiers(&[enc]));
        put(&mut r, "type", self.encounter_type(code).map(|c| json!([c])));
        put(&mut r, "subject", self.subject());
        put(&mut r, "period", ordered_period(child(enc, "effectiveTime"), enc, rep));
        let participants: Vec<_> = children(enc, "performer")
            .filter_map(|p| {
                let individual = child(p, "assignedEntity").and_then(assigned_reference)?;
                let role = attr(p, "typeCode").unwrap_or("PPRF");
                Some(json!({
                    "type": [coded("http://terminology.hl7.org/CodeSystem/v3-ParticipationType", role)],
                    "individual": individual,
                }))
            })
            .collect();
        put(
            &mut r,
            "participant",
            (!participants.is_empty()).then(|| json!(participants)),
        );
        let locations: Vec<_> = children(enc, "participant")
            .filter(|p| attr(*p, "typeCode") == Some("LOC"))
            .filter_map(|p| path(p, &["participantRole", "playingEntity", "name"]).and_then(xml::text))
            .map(|display| json!({"location": {"display": display}}))
            .collect();
        put(&mut r, "location", (!locations.is_empty()).then(|| json!(locations)));
        put(&mut r, "reasonCode", (!reasons.is_empty()).then(|| json!(reasons)));
        if children(enc, "text").any(has_words) {
            rep.not_read.push("encounter (text kept in the uploaded file)".into());
        }
        let reference = path(enc, &["text", "reference"])
            .and_then(|r| attr(r, "value"))
            .filter(|r| r.starts_with('#') && r.len() <= 256);
        put(
            &mut r,
            "extension",
            reference.map(|value| {
                json!([{
                    "url": "https://medplum.com/fhir/StructureDefinition/ccda-narrative-reference",
                    "valueString": value
                }])
            }),
        );
        self.push(Some(rep), at, Value::Object(r));
        true
    }

    /// The visit type, named by the words the document shows for it (its original text) when it has
    /// them: a CPT display name is a paragraph of billing rules.
    fn encounter_type(&mut self, code: Option<Node<'_, '_>>) -> Option<Value> {
        self.reported_concept(code, &[], true)
    }

    pub(super) fn procedure(&mut self, n: Node<'_, '_>, at: &str, rep: &mut SectionReport) -> bool {
        let act = n.has_tag_name((xml::NS, "act"));
        let template = if act { PROCEDURE_ACT } else { PROCEDURE_ACTIVITY };
        if !event(n, &[template]) {
            return false;
        }
        // Procedures that were negated, cancelled, unstarted or nullified wait in review.
        if is_true(n, "negationInd") {
            let line = format!("{} (not done)", self.hold(n));
            rep.not_read.push(line);
            return true;
        }
        let status = match status_of(n) {
            Some("active") => "in-progress",
            Some("aborted") => "stopped",
            Some(s @ ("cancelled" | "new" | "nullified")) => {
                let line = format!("{} (status {s})", self.hold(n));
                rep.not_read.push(line);
                return true;
            }
            Some("held" | "suspended") => "on-hold",
            None | Some("completed") => "completed",
            _ => "unknown",
        };
        let Some(code) = self.concept(child(n, "code")) else {
            return false;
        };
        let id = self.new_id("Procedure", &[n], n);
        let mut r = Map::from_iter([
            ("resourceType".into(), json!("Procedure")),
            ("id".into(), json!(id)),
            ("status".into(), json!(status)),
            ("code".into(), code),
        ]);
        put(&mut r, "identifier", Self::identifiers(&[n]));
        put(&mut r, "subject", self.subject());
        let time = child(n, "effectiveTime");
        if let Some(date) = attr_ts(time).filter(|_| act) {
            r.insert("performedDateTime".into(), json!(date));
        } else {
            put(&mut r, "performedPeriod", ordered_period(time, n, rep));
        }
        let sites: Vec<_> = children(n, "targetSiteCode")
            .filter_map(|c| self.concept(Some(c)))
            .collect();
        put(&mut r, "bodySite", (!sites.is_empty()).then(|| json!(sites)));
        let author = path(n, &["author", "assignedAuthor"]).and_then(assigned_reference);
        put(&mut r, "recorder", author.clone());
        put(&mut r, "asserter", author);
        let performers: Vec<_> = children(n, "performer")
            .filter_map(|p| child(p, "assignedEntity").and_then(assigned_reference))
            .map(|actor| json!({"actor": actor}))
            .collect();
        put(&mut r, "performer", (!performers.is_empty()).then(|| json!(performers)));
        put(&mut r, "note", self.activity_note(n, rep));
        self.push(Some(rep), at, Value::Object(r));
        self.unread_relationships(n, rep);
        true
    }

    pub(super) fn activity_observation(&mut self, obs: Node<'_, '_>, at: &str, rep: &mut SectionReport) -> bool {
        let (roots, category) = if rep.kind == Some(Section::SocialHistory) {
            if has_template(obs, EXCLUDED_SOCIAL_OBSERVATIONS) {
                return false;
            }
            (SOCIAL_OBSERVATIONS, "social-history")
        } else {
            (&[PROCEDURE_OBSERVATION][..], "procedure")
        };
        if !event(obs, roots) || is_true(obs, "negationInd") {
            return false;
        }
        // The FHIR mapper skips these statuses. Hold the activity and its text for review.
        if let Some(s @ ("aborted" | "cancelled" | "nullified")) = status_of(obs) {
            let line = format!("{} (status {s})", self.hold(obs));
            rep.not_read.push(line);
            return true;
        }
        let read = self.observation(obs, None, category, at, rep).is_some();
        if read {
            self.unread_relationships(obs, rep);
        }
        read
    }

    /// The activity's bounded note, with an incomplete scan listed even if no words fit.
    pub(super) fn activity_note(&mut self, n: Node<'_, '_>, rep: &mut SectionReport) -> Option<Value> {
        let note = self.narrative_of(child(n, "text")?);
        if note.truncated {
            rep.not_read.push(format!("{} (note scan limit reached)", describe(n)));
        }
        note.value.map(|text| json!([{"text": text}]))
    }
}
