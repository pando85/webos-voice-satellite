//! The input and app lists behind the `tv_input` and `tv_app` selects
//! (`docs/design/tv-control.md` section 8).

use crate::luna::{ExternalInput, LaunchPoint};

/// The option HA sees when the foreground app is not in the list. Choosing it
/// from HA is refused.
pub const OTHER: &str = "Other";

/// An input as the select shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    /// The TV's input `id` (what the setter takes).
    pub id: String,
    /// Select option: the TV's label, made unique.
    pub label: String,
    /// The app that shows the input; the state follows the foreground app.
    pub app_id: Option<String>,
}

/// An app as the select shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// The app id (what `launch` takes).
    pub id: String,
    /// Select option: the launcher title, made unique.
    pub title: String,
}

/// Makes `wanted` unique among `taken` (and different from [`OTHER`]) with a
/// counter: `Name`, `Name 2`, `Name 3`.
fn unique(taken: &[String], wanted: &str) -> String {
    let free = |s: &str| s != OTHER && !taken.iter().any(|t| t == s);
    if free(wanted) {
        return wanted.to_owned();
    }
    (2..10_000)
        .map(|n| format!("{wanted} {n}"))
        .find(|c| free(c))
        .unwrap_or_else(|| wanted.to_owned())
}

/// All entries of the TV's external input list, in the TV's order, connected
/// or not, by label. A duplicate label gets the input `id` appended (and a
/// counter if that is still not unique). `subList` is ignored in v1.
#[must_use]
pub fn build_inputs(devices: &[ExternalInput]) -> Vec<Input> {
    let mut out: Vec<Input> = Vec::new();
    for d in devices {
        let taken: Vec<String> = out.iter().map(|i| i.label.clone()).collect();
        let label = if taken.contains(&d.label) || d.label == OTHER {
            unique(&taken, &format!("{} ({})", d.label, d.id))
        } else {
            d.label.clone()
        };
        out.push(Input {
            id: d.id.clone(),
            label,
            app_id: d.app_id.clone(),
        });
    }
    out
}

/// The app list: launch points the launcher shows, without the satellite's
/// own app and without the input apps (matched by exact app id, not by name
/// patterns), in the launcher's order, titles made unique, at most `limit`.
#[must_use]
pub fn build_apps(
    points: &[LaunchPoint],
    own_app_ids: &[String],
    input_app_ids: &[String],
    limit: usize,
) -> Vec<App> {
    let mut out: Vec<App> = Vec::new();
    for p in points {
        if out.len() >= limit {
            break;
        }
        if p.hidden
            || own_app_ids.contains(&p.id)
            || input_app_ids.contains(&p.id)
            || out.iter().any(|a| a.id == p.id)
        {
            continue;
        }
        let taken: Vec<String> = out.iter().map(|a| a.title.clone()).collect();
        out.push(App {
            id: p.id.clone(),
            title: unique(&taken, &p.title),
        });
    }
    out
}

/// The select options for a list of labels: the labels, then [`OTHER`].
#[must_use]
pub fn options<'a>(labels: impl Iterator<Item = &'a str>) -> Vec<String> {
    labels
        .map(str::to_owned)
        .chain(std::iter::once(OTHER.to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn dev(id: &str, label: &str, app: Option<&str>) -> ExternalInput {
        ExternalInput {
            id: id.to_owned(),
            label: label.to_owned(),
            app_id: app.map(str::to_owned),
            connected: false,
        }
    }

    fn lp(id: &str, title: &str, hidden: bool) -> LaunchPoint {
        LaunchPoint {
            id: id.to_owned(),
            title: title.to_owned(),
            hidden,
        }
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn inputs_keep_order_and_disambiguate_duplicate_labels() {
        let l = build_inputs(&[
            dev("HDMI_1", "HDMI 1", Some("app.hdmi1")),
            dev("HDMI_2", "Console", None),
            dev("HDMI_3", "Console", None),
            dev("AV_1", "Other", None),
        ]);
        let labels: Vec<_> = l.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            ["HDMI 1", "Console", "Console (HDMI_3)", "Other (AV_1)"]
        );
        assert_eq!(l[0].app_id.as_deref(), Some("app.hdmi1"));
        assert_eq!(build_inputs(&[]), []);
        // A still-duplicate label gets a counter.
        let l = build_inputs(&[
            dev("A", "X", None),
            dev("A", "X", None),
            dev("A", "X", None),
        ]);
        let labels: Vec<_> = l.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["X", "X (A)", "X (A) 2"]);
    }

    #[test]
    fn apps_filter_dedupe_order_and_cap() {
        let points = [
            lp("app.a", "App A", false),
            lp("app.hidden", "Hidden", true),
            lp("com.own.app", "Voice Satellite", false),
            lp("app.hdmi1", "HDMI 1", false),
            lp("app.b", "App B", false),
            lp("app.b2", "App B", false),
            lp("app.a", "App A again", false),
            lp("app.o", "Other", false),
            lp("app.c", "App C", false),
        ];
        let apps = build_apps(
            &points,
            &strings(&["com.own.app"]),
            &strings(&["app.hdmi1"]),
            30,
        );
        let titles: Vec<_> = apps.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, ["App A", "App B", "App B 2", "Other 2", "App C"]);
        assert!(
            apps.iter()
                .all(|a| a.id != "app.hidden" && a.id != "com.own.app")
        );
        // The cap counts kept apps only.
        let capped = build_apps(
            &points,
            &strings(&["com.own.app"]),
            &strings(&["app.hdmi1"]),
            2,
        );
        assert_eq!(capped.len(), 2);
        assert_eq!(capped[1].title, "App B");
        assert_eq!(build_apps(&[], &[], &[], 30), []);
    }

    #[test]
    fn options_end_with_other() {
        assert_eq!(options(["A", "B"].into_iter()), ["A", "B", OTHER]);
        assert_eq!(options(std::iter::empty()), [OTHER]);
    }

    #[test]
    fn a_large_generated_list_is_capped() {
        let points: Vec<_> = (0..2000)
            .map(|n| lp(&format!("app.{n}"), "Same title", false))
            .collect();
        let apps = build_apps(&points, &[], &[], 30);
        assert_eq!(apps.len(), 30);
        let mut titles: Vec<_> = apps.iter().map(|a| a.title.clone()).collect();
        titles.sort();
        titles.dedup();
        assert_eq!(titles.len(), 30, "every title is unique");
    }
}
